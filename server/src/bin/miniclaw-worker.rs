// Connects guest Codex through private VM channels.
#[cfg(unix)]
use miniclaw_server::{channel::Stream, remote::Settings, runtime, worker, computer,
    guest_computer};
// Reads guest paths and startup failure results.
#[cfg(unix)]
use std::{error::Error, net::Ipv4Addr, path::PathBuf, time::Duration};
// Bounds proxy connections and waits for shutdown.
#[cfg(unix)]
use tokio::{net::TcpListener, sync::Semaphore};
// Shares the guest-only proxy admission limit.
#[cfg(unix)]
use std::sync::Arc;

/// Runs only inside an Apple silicon macOS guest under its non-root user.
#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let model = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.model"])
        .output()?;
    if !String::from_utf8_lossy(&model.stdout)
        .trim()
        .starts_with("VirtualMac")
        || rustix::process::getuid().is_root()
        || !cfg!(target_arch = "aarch64")
    {
        return Err("Worker must run as a non-root user inside the macOS VM".into());
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--computer-filter") && args.len() == 1 {
        // A VM is already the sandbox: inside it an agent may use any app.
        return computer::filter(false).await.map_err(Into::into);
    }
    if args.first().is_some_and(|arg| arg == "--computer-helper") && args.len() == 2 {
        return computer::bridge(&args[1]).map_err(Into::into);
    }
    if args.len() != 1 {
        return Err("Expected the private worker settings path".into());
    }
    let settings = Settings::read(&PathBuf::from(&args[0]))?;
    let cwd = PathBuf::from(&settings.cwd);
    if cwd.canonicalize()? != cwd {
        return Err("Guest workspace must not be a symlink".into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("Missing guest home")?);
    let binary = home.join("Library/Application Support/MiniClaw/Runtime/codex-0.154.0/bin/codex");
    let binary = binary.to_str().ok_or("Invalid guest runtime path")?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let proxy = format!("http://{}", listener.local_addr()?);
    let forwarding = tokio::spawn(proxy_connections(listener));
    let work = async {
        loop {
            let mut command = runtime::worker_command(binary, &settings.cwd)
                .await
                .map_err(|_| "Guest Codex configuration failed validation")?;
            for name in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "WS_PROXY",
                "WSS_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
                "ws_proxy",
                "wss_proxy",
            ] {
                command.env(name, &proxy);
            }
            command
                .env("NO_PROXY", "localhost,127.0.0.1,::1")
                .env("no_proxy", "localhost,127.0.0.1,::1");
            if let Ok(stream) = Stream::host(10242).await {
                let policy = guest_computer::Policy::new(computer::helper());
                let _ = worker::serve_with(stream, settings.key, || {
                    println!("worker_connected");
                    command
                }, Some(policy))
                .await;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        #[allow(unreachable_code)]
        Ok::<(), Box<dyn Error>>(())
    };
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let result = tokio::select! {
        result = work => result,
        _ = term.recv() => Ok(()),
        _ = tokio::signal::ctrl_c() => Ok(()),
    };
    forwarding.abort();
    let _ = forwarding.await;
    result
}

/// Sends guest-loopback proxy connections only to the fixed host egress service.
#[cfg(unix)]
async fn proxy_connections(listener: TcpListener) {
    let slots = Arc::new(Semaphore::new(16));
    while let Ok((mut client, _)) = listener.accept().await {
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue;
        };
        tokio::spawn(async move {
            let _slot = slot;
            if let Ok(mut stream) = Stream::host(10243).await {
                let _ = tokio::io::copy_bidirectional(&mut client, &mut stream).await;
            }
        });
    }
}

/// Windows builds carry no VM worker or fallback host execution.
#[cfg(windows)]
fn main() {
    eprintln!("MiniClaw VM workers are not supported on Windows.");
    std::process::exit(1);
}
