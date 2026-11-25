//! Fake IO for the unit tests: a peer that breaks in a known way.

/// An IO whose reads never finish and whose writes always fail.
pub(crate) struct Broken;

impl tokio::io::AsyncRead for Broken {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Pending
    }
}

impl tokio::io::AsyncWrite for Broken {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(Err(std::io::Error::other("pipe broke")))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// An IO whose reads and flushes fail; a write is taken whole.
pub(crate) struct Unreadable;

impl tokio::io::AsyncRead for Unreadable {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::other("reset")))
    }
}

impl tokio::io::AsyncWrite for Unreadable {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::other("flush failed")))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn the_fakes_break_as_documented() {
    use futures::FutureExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut buf = [0u8; 1];
    assert!(
        Broken.read(&mut buf).now_or_never().is_none(),
        "a read never finishes"
    );
    assert!(Broken.write(b"x").await.is_err());
    assert!(Broken.flush().await.is_ok() && Broken.shutdown().await.is_ok());
    assert!(Unreadable.read(&mut buf).await.is_err());
    assert_eq!(Unreadable.write(b"x").await.unwrap(), 1);
    assert!(Unreadable.flush().await.is_err() && Unreadable.shutdown().await.is_ok());
}
