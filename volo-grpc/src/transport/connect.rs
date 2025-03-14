use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use futures_util::future::BoxFuture;
use hyper::rt::ReadBufCursor;
use hyper_util::client::legacy::connect::{Connected, Connection};
use motore::{make::MakeConnection, service::UnaryService};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
#[cfg(feature = "__tls")]
#[cfg_attr(docsrs, doc(cfg(any(feature = "rustls", feature = "native-tls"))))]
use volo::net::tls::{ClientTlsConfig, TlsMakeTransport};
use volo::net::{
    conn::Conn,
    dial::{Config, DefaultMakeTransport, MakeTransport},
    Address,
};

tokio::task_local! {
    pub static ADDRESS_HINT: Address;
}

#[derive(Clone, Debug)]
pub struct Connector {
    inner: ConnectorInner,
}

#[derive(Clone, Debug)]
pub enum ConnectorInner {
    Default(DefaultMakeTransport),
    #[cfg(feature = "__tls")]
    #[cfg_attr(docsrs, doc(cfg(any(feature = "rustls", feature = "native-tls"))))]
    Tls(TlsMakeTransport),
}

impl Connector {
    pub fn new(cfg: Option<Config>) -> Self {
        let mut mt = DefaultMakeTransport::default();
        if let Some(cfg) = cfg {
            mt.set_connect_timeout(cfg.connect_timeout);
            mt.set_read_timeout(cfg.read_timeout);
            mt.set_write_timeout(cfg.write_timeout);
        }
        Self {
            inner: ConnectorInner::Default(mt),
        }
    }

    #[cfg(feature = "__tls")]
    #[cfg_attr(docsrs, doc(cfg(any(feature = "rustls", feature = "native-tls"))))]
    pub fn new_with_tls(cfg: Option<Config>, tls_config: ClientTlsConfig) -> Self {
        let mut mt = TlsMakeTransport::new(cfg.unwrap_or_default(), tls_config);
        if let Some(cfg) = cfg {
            mt.set_connect_timeout(cfg.connect_timeout);
            mt.set_read_timeout(cfg.read_timeout);
            mt.set_write_timeout(cfg.write_timeout);
        }
        Self {
            inner: ConnectorInner::Tls(mt),
        }
    }
}

impl Default for Connector {
    fn default() -> Self {
        Self::new(None)
    }
}

impl UnaryService<Address> for Connector {
    type Response = Conn;
    type Error = io::Error;

    async fn call(&self, addr: Address) -> Result<Self::Response, Self::Error> {
        match &self.inner {
            ConnectorInner::Default(mkt) => mkt.make_connection(addr).await,
            #[cfg(feature = "__tls")]
            ConnectorInner::Tls(mkt) => mkt.make_connection(addr).await,
        }
    }
}

impl tower::Service<http::Uri> for Connector {
    type Response = ConnectionWrapper;

    type Error = io::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: http::Uri) -> Self::Future {
        let connector = self.clone();
        Box::pin(async move {
            let target = ADDRESS_HINT.get();
            Ok(ConnectionWrapper {
                inner: connector.make_connection(target).await?,
            })
        })
    }
}

#[pin_project::pin_project]
pub struct ConnectionWrapper {
    #[pin]
    inner: Conn,
}

impl hyper::rt::Read for ConnectionWrapper {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: ReadBufCursor<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let n = unsafe {
            let mut tbuf = tokio::io::ReadBuf::uninit(buf.as_mut());
            match tokio::io::AsyncRead::poll_read(self.project().inner, cx, &mut tbuf) {
                Poll::Ready(Ok(())) => tbuf.filled().len(),
                other => return other,
            }
        };

        unsafe {
            buf.advance(n);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for ConnectionWrapper {
    #[inline]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for ConnectionWrapper {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl AsyncWrite for ConnectionWrapper {
    #[inline]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    #[inline]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    #[inline]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Connection for ConnectionWrapper {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

#[cfg(test)]
mod tests {
    use hex::FromHex;

    #[test]
    fn test_convert() {
        let authority = "2f746d702f7270632e736f636b";
        assert_eq!(
            String::from_utf8(Vec::from_hex(authority).unwrap()).unwrap(),
            "/tmp/rpc.sock"
        );
    }
}
