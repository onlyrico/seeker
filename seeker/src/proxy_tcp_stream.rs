use async_std::io::{Read, Write};
use async_std::net::TcpStream;
use config::{Address, ServerConfig, ServerProtocol};
use http_proxy_client::{HttpProxyTcpStream, HttpsProxyTcpStream};
use socks5_client::Socks5TcpStream;
use ssclient::SSTcpStream;
use std::io::Result;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::dns_client::DnsClient;
use std::io::{Error, ErrorKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone)]
enum ProxyTcpStreamInner {
    Direct(TcpStream),
    Socks5(Socks5TcpStream),
    HttpProxy(HttpProxyTcpStream),
    HttpsProxy(HttpsProxyTcpStream),
    Shadowsocks(SSTcpStream),
}

#[derive(Clone)]
pub struct ProxyTcpStream {
    inner: ProxyTcpStreamInner,
    alive: Arc<AtomicBool>,
}

impl ProxyTcpStream {
    pub async fn connect(
        remote_addr: Address,
        config: Option<&ServerConfig>,
        alive: Arc<AtomicBool>,
        dns_client: DnsClient,
    ) -> Result<ProxyTcpStream> {
        let stream = if let Some(config) = config {
            match config.protocol() {
                ServerProtocol::Https => {
                    let proxy_socket_addr = dns_client.lookup_address(config.addr()).await?;
                    let proxy_hostname = match config.addr().hostname() {
                        None => {
                            return Err(Error::new(
                                ErrorKind::InvalidData,
                                "proxy domain must not be empty for https protocol.",
                            ))
                        }
                        Some(s) => s,
                    };
                    ProxyTcpStreamInner::HttpsProxy(
                        HttpsProxyTcpStream::connect(
                            proxy_socket_addr,
                            proxy_hostname.to_string(),
                            remote_addr,
                            config.username(),
                            config.password(),
                        )
                        .await?,
                    )
                }
                ServerProtocol::Http => {
                    let proxy_socket_addr = dns_client.lookup_address(config.addr()).await?;
                    ProxyTcpStreamInner::HttpProxy(
                        HttpProxyTcpStream::connect(
                            proxy_socket_addr,
                            remote_addr,
                            config.username(),
                            config.password(),
                        )
                        .await?,
                    )
                }
                ServerProtocol::Socks5 => {
                    let proxy_socket_addr = dns_client.lookup_address(config.addr()).await?;
                    ProxyTcpStreamInner::Socks5(
                        Socks5TcpStream::connect(proxy_socket_addr, remote_addr).await?,
                    )
                }
                ServerProtocol::Shadowsocks => {
                    let proxy_socket_addr = dns_client.lookup_address(config.addr()).await?;
                    let (method, key) = match (config.method(), config.key()) {
                        (Some(m), Some(k)) => (m, k),
                        _ => {
                            return Err(Error::new(
                                ErrorKind::InvalidData,
                                "method and password must be set for ss protocol.",
                            ))
                        }
                    };
                    ProxyTcpStreamInner::Shadowsocks(
                        SSTcpStream::connect(proxy_socket_addr, remote_addr, method, key).await?,
                    )
                }
            }
        } else {
            let socket_addr = dns_client.lookup_address(&remote_addr).await?;
            ProxyTcpStreamInner::Direct(TcpStream::connect(socket_addr).await?)
        };

        Ok(ProxyTcpStream {
            inner: stream,
            alive,
        })
    }
}

impl Read for ProxyTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let stream = &mut *self;
        if !stream.alive.load(Ordering::SeqCst) {
            return Poll::Ready(Err(Error::new(
                ErrorKind::BrokenPipe,
                "ProxyTcpStream not alive",
            )));
        }
        match &mut stream.inner {
            ProxyTcpStreamInner::Direct(conn) => Pin::new(conn).poll_read(cx, buf),
            ProxyTcpStreamInner::Socks5(conn) => Pin::new(conn).poll_read(cx, buf),
            ProxyTcpStreamInner::Shadowsocks(conn) => Pin::new(conn).poll_read(cx, buf),
            ProxyTcpStreamInner::HttpProxy(conn) => Pin::new(conn).poll_read(cx, buf),
            ProxyTcpStreamInner::HttpsProxy(conn) => Pin::new(conn).poll_read(cx, buf),
        }
    }
}

impl Write for ProxyTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        let stream = &mut *self;
        if !stream.alive.load(Ordering::SeqCst) {
            return Poll::Ready(Err(Error::new(
                ErrorKind::BrokenPipe,
                "ProxyTcpStream not alive",
            )));
        }
        match &mut stream.inner {
            ProxyTcpStreamInner::Direct(conn) => Pin::new(conn).poll_write(cx, buf),
            ProxyTcpStreamInner::Socks5(conn) => Pin::new(conn).poll_write(cx, buf),
            ProxyTcpStreamInner::Shadowsocks(conn) => Pin::new(conn).poll_write(cx, buf),
            ProxyTcpStreamInner::HttpProxy(conn) => Pin::new(conn).poll_write(cx, buf),
            ProxyTcpStreamInner::HttpsProxy(conn) => Pin::new(conn).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let stream = &mut *self;
        if !stream.alive.load(Ordering::SeqCst) {
            return Poll::Ready(Err(Error::new(
                ErrorKind::BrokenPipe,
                "ProxyTcpStream not alive",
            )));
        }
        match &mut stream.inner {
            ProxyTcpStreamInner::Direct(conn) => Pin::new(conn).poll_flush(cx),
            ProxyTcpStreamInner::Socks5(conn) => Pin::new(conn).poll_flush(cx),
            ProxyTcpStreamInner::Shadowsocks(conn) => Pin::new(conn).poll_flush(cx),
            ProxyTcpStreamInner::HttpProxy(conn) => Pin::new(conn).poll_flush(cx),
            ProxyTcpStreamInner::HttpsProxy(conn) => Pin::new(conn).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let stream = &mut *self;
        if !stream.alive.load(Ordering::SeqCst) {
            return Poll::Ready(Err(Error::new(
                ErrorKind::BrokenPipe,
                "ProxyTcpStream not alive",
            )));
        }
        match &mut stream.inner {
            ProxyTcpStreamInner::Direct(conn) => Pin::new(conn).poll_close(cx),
            ProxyTcpStreamInner::Socks5(conn) => Pin::new(conn).poll_close(cx),
            ProxyTcpStreamInner::Shadowsocks(conn) => Pin::new(conn).poll_close(cx),
            ProxyTcpStreamInner::HttpProxy(conn) => Pin::new(conn).poll_close(cx),
            ProxyTcpStreamInner::HttpsProxy(conn) => Pin::new(conn).poll_close(cx),
        }
    }
}
