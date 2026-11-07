// Owns native sockets without network listeners.
use socket2::{Domain, SockAddr, Socket, Type};
// Adapts descriptor operations to asynchronous polling.
use std::{
    io::{self, Read, Write},
    net::Shutdown,
    pin::Pin,
    task::{Context, Poll, ready},
};
// Registers descriptor readiness with the reactor.
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd};
// Defines platform-specific virtual socket address layouts.
use libc::{AF_VSOCK, sockaddr_vm};

pub struct Stream(AsyncFd<Socket>);

impl Stream {
    /// Takes ownership of a nonblocking socket and registers its readiness events.
    pub fn new(socket: Socket) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        #[cfg(target_os = "macos")]
        socket.set_nosigpipe(true)?;
        Ok(Self(AsyncFd::new(socket)?))
    }

    /// Dials only the host virtual CID, with a five-second connection deadline.
    pub async fn host(port: u32) -> io::Result<Self> {
        let socket = Socket::new(Domain::from(AF_VSOCK), Type::STREAM, None)?;
        socket.set_nonblocking(true)?;
        let address = address(port);
        if let Err(error) = socket.connect(&address)
            && error.raw_os_error() != Some(libc::EINPROGRESS)
            && error.kind() != io::ErrorKind::WouldBlock
        {
            return Err(error);
        }
        let stream = Self::new(socket)?;
        drop(
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.0.writable())
                .await
                .map_err(|_| io::ErrorKind::TimedOut)??,
        );
        if let Some(error) = stream.0.get_ref().take_error()? {
            return Err(error);
        }
        Ok(stream)
    }
}

impl AsyncRead for Stream {
    /// Reads available bytes without blocking the runtime thread.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().read(buf.initialize_unfilled())) {
                Ok(result) => {
                    buf.advance(result?);
                    return Poll::Ready(Ok(()));
                }
                Err(_) => continue,
            }
        }
    }
}

impl AsyncWrite for Stream {
    /// Writes available bytes while preserving backpressure from the peer.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }

    /// Completes immediately because this adapter has no write buffer.
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    /// Closes the write half while retaining pending replies from the peer.
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.0.get_ref().shutdown(Shutdown::Write))
    }
}

/// Builds the platform's native address for host CID 2 and the fixed service port.
fn address(port: u32) -> SockAddr {
    // The native struct's zeroed reserved fields and family match its declared length.
    unsafe {
        let mut addr: sockaddr_vm = std::mem::zeroed();
        addr.svm_family = AF_VSOCK as _;
        addr.svm_cid = 2;
        addr.svm_port = port;
        #[cfg(target_os = "macos")]
        {
            addr.svm_len = std::mem::size_of::<sockaddr_vm>() as _;
        }
        SockAddr::try_init(|storage, len| {
            storage.cast::<sockaddr_vm>().write(addr);
            *len = std::mem::size_of::<sockaddr_vm>() as _;
            Ok(())
        })
        .expect("native virtual socket address")
        .1
    }
}
