//! A per-command outbound proxy. A sandboxed shell command has no network; when the user
//! approved hosts for it, this proxy is its only way out. It opens HTTPS connections
//! (CONNECT to port 443) to exactly those hosts and refuses everything else.

// Shares the allowlist and the refusal record with each connection.
use std::{future::Future, net::{IpAddr, SocketAddr}, pin::Pin, sync::{Arc, Mutex}};
// Accepts the command's connections and relays approved ones.
use tokio::{io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt}, net::{TcpListener, TcpStream}};

/// Turns an approved host name into the addresses the proxy may connect to.
pub type Resolver = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Vec<SocketAddr>> + Send>>
    + Send + Sync>;

/// Reports an address a public host name has no business resolving to. An approved name
/// must not become a way to reach this Mac, the VM host or the local network.
pub fn private(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local()
            || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast()
            || ip.octets()[0] == 0 || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unspecified() || ip.is_multicast()
            || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80
            || ip.to_ipv4_mapped().is_some_and(|mapped| private(&IpAddr::V4(mapped))),
    }
}

/// Resolves through the system and keeps public addresses only.
pub fn system_resolver() -> Resolver {
    Arc::new(|host: String| Box::pin(async move {
        tokio::net::lookup_host((host.as_str(), 443)).await
            .map(|found| found.filter(|address| !private(&address.ip())).collect())
            .unwrap_or_default()
    }))
}

/// One running proxy. Dropping it stops the listener.
pub struct Proxy {
    pub port: u16,
    refused: Arc<Mutex<Vec<String>>>,
    /// The hosts a connection may reach; a shared shell's proxy changes them per command.
    hosts: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
    /// On Linux the sandbox has its own network, so it reaches the proxy through this socket.
    #[cfg(target_os = "linux")]
    unix: Option<(tokio::task::JoinHandle<()>, crate::executor::Scratch)>,
}


impl Proxy {
    /// Listens on a free loopback port for the lifetime of one command.
    pub async fn start(hosts: Vec<String>, resolve: Resolver) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let refused = Arc::new(Mutex::new(Vec::new()));
        let hosts = Arc::new(Mutex::new(hosts));
        let (allowed, record) = (hosts.clone(), refused.clone());
        #[cfg(target_os = "linux")]
        let resolve_unix = resolve.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                // Each connection reads the list as it is now, so a change applies to the next one.
                let now = Arc::new(allowed.lock().map(|list| list.clone()).unwrap_or_default());
                tokio::spawn(serve(stream, now, resolve.clone(), record.clone()));
            }
        });
        #[cfg(target_os = "linux")]
        let unix = {
            // A private folder holds the socket, so only this user can connect to it.
            let folder = crate::executor::Scratch::create()?;
            let path = std::path::Path::new(&folder.0).join("egress.sock");
            let listener = tokio::net::UnixListener::bind(&path)?;
            let (allowed, record, resolve) = (hosts.clone(), refused.clone(), resolve_unix);
            let task = tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let now = Arc::new(allowed.lock().map(|list| list.clone()).unwrap_or_default());
                    tokio::spawn(serve(stream, now, resolve.clone(), record.clone()));
                }
            });
            crate::sandbox_linux::register(port, path);
            Some((task, folder))
        };
        Ok(Self { port, refused, hosts, task, #[cfg(target_os = "linux")] unix })
    }

    /// Replaces the hosts later connections may reach.
    pub fn set_hosts(&self, hosts: Vec<String>) {
        if let Ok(mut list) = self.hosts.lock() { *list = hosts; }
    }

    /// Returns and forgets the refused destinations, so each command reports its own.
    pub fn take_refused(&self) -> Vec<String> {
        self.refused.lock().map(|mut list| std::mem::take(&mut *list)).unwrap_or_default()
    }

    /// Names every destination that was refused, in order, without repeats.
    pub fn refused(&self) -> Vec<String> {
        self.refused.lock().map(|list| list.clone()).unwrap_or_default()
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
        #[cfg(target_os = "linux")]
        if let Some((task, folder)) = self.unix.take() {
            task.abort();
            crate::sandbox_linux::unregister(&std::path::Path::new(&folder.0).join("egress.sock"));
        }
    }
}

/// Handles one connection: a CONNECT to an approved host on port 443, or a refusal.
async fn serve(mut client: impl AsyncRead + AsyncWrite + Unpin, hosts: Arc<Vec<String>>, resolve: Resolver,
    refused: Arc<Mutex<Vec<String>>>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    // The request head is read byte by byte so nothing of the tunnel is consumed.
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= 8192 || !matches!(client.read(&mut byte).await, Ok(1)) { return; }
        head.push(byte[0]);
    }
    let line = String::from_utf8_lossy(&head);
    let mut words = line.lines().next().unwrap_or_default().split(' ');
    let (method, target) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
    let (host, port) = target.rsplit_once(':').unwrap_or((target, ""));
    let host = host.to_ascii_lowercase();
    let allowed = method == "CONNECT" && port == "443" && hosts.contains(&host);
    let addresses = if allowed { resolve(host.clone()).await } else { Vec::new() };
    let mut upstream = None;
    for address in addresses {
        if let Ok(stream) = TcpStream::connect(address).await { upstream = Some(stream); break; }
    }
    let Some(mut upstream) = upstream else {
        let name = if port == "443" || port.is_empty() { host } else { format!("{host}:{port}") };
        if let Ok(mut list) = refused.lock() && !list.contains(&name) && list.len() < 32 {
            list.push(name);
        }
        let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n").await;
        return;
    };
    if client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err() { return; }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A public name must never lead to a local, private or link-local address.
    #[test]
    fn local_and_private_addresses_are_refused() {
        for address in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254",
            "0.0.0.0", "100.64.0.1", "::1", "fe80::1", "fd00::1", "::ffff:10.0.0.1"] {
            assert!(private(&address.parse().unwrap()), "{address}");
        }
        for address in ["1.1.1.1", "140.82.112.3", "2606:4700:4700::1111"] {
            assert!(!private(&address.parse().unwrap()), "{address}");
        }
    }
}
