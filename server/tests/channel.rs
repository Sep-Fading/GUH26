#![cfg(unix)]

// Exercises private descriptor IO and ownership.
use miniclaw_server::channel::Stream;
// Supplies kernel sockets without public listeners.
use socket2::{Domain, Socket, Type};
// Checks async reads, writes and shutdown.
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Preserves byte streams and EOF through the adapter used for virtual sockets.
#[tokio::test]
async fn streams_and_closes() {
    let (left, right) = Socket::pair(Domain::UNIX, Type::STREAM, None).unwrap();
    let mut left = Stream::new(left).unwrap();
    let mut right = Stream::new(right).unwrap();
    let sending = tokio::spawn(async move {
        left.write_all(&vec![42; 200_000]).await.unwrap();
        left.shutdown().await.unwrap();
        let mut reply = [0; 2];
        left.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [1, 2]);
    });
    let mut bytes = Vec::new();
    right.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, vec![42; 200_000]);
    right.write_all(&[1, 2]).await.unwrap();
    sending.await.unwrap();
}

/// Dropping the stream releases the peer even while a read is waiting.
#[tokio::test]
async fn drop_closes_peer() {
    let (left, right) = Socket::pair(Domain::UNIX, Type::STREAM, None).unwrap();
    let left = Stream::new(left).unwrap();
    let mut right = Stream::new(right).unwrap();
    drop(left);
    assert_eq!(right.read(&mut [0; 8]).await.unwrap(), 0);
}
