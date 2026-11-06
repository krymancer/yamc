//! IMAP connections over TCP, implicit TLS or STARTTLS (rustls).

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::crypto::{CryptoProvider, ring, verify_tls12_signature, verify_tls13_signature};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

use crate::types::{AccountConfig, ServerConfig, TlsMode};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const LOOPBACK: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

/// Self-signed certificates are only accepted from this machine (Proton Bridge).
pub fn insecure_ok(cfg: &AccountConfig, server: &ServerConfig) -> bool {
    cfg.allow_self_signed && LOOPBACK.contains(&server.host.as_str())
}

#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, tokio_rustls::rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub fn tls_config(insecure: bool) -> Result<Arc<ClientConfig>> {
    let provider = Arc::new(ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions()?;
    let cfg = if insecure {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(AcceptAny(provider))).with_no_client_auth()
    } else {
        let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Ok(Arc::new(cfg))
}

pub enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Stream::Plain(_) => "Stream::Plain",
            Stream::Tls(_) => "Stream::Tls",
        })
    }
}

impl AsyncRead for Stream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

pub type Session = async_imap::Session<Stream>;

async fn tls_wrap(tcp: TcpStream, host: &str, insecure: bool) -> Result<Stream> {
    let name = ServerName::try_from(host.to_string()).map_err(|_| anyhow!("bad host name {host}"))?;
    let tls = TlsConnector::from(tls_config(insecure)?).connect(name, tcp).await.context("TLS handshake")?;
    Ok(Stream::Tls(Box::new(tls)))
}

/// Connect and log in. Errors read like "IMAP: …" so the settings screen can show them as-is.
pub async fn imap_connect(cfg: &AccountConfig, password: &str) -> Result<Session> {
    let s = &cfg.imap;
    let insecure = insecure_ok(cfg, s);
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((s.host.as_str(), s.port)))
        .await
        .map_err(|_| anyhow!("timed out connecting to {}:{}", s.host, s.port))?
        .with_context(|| format!("can't reach {}:{}", s.host, s.port))?;
    let _ = tcp.set_nodelay(true);
    let client = match s.tls {
        TlsMode::Tls => greeted(async_imap::Client::new(tls_wrap(tcp, &s.host, insecure).await?)).await?,
        TlsMode::None => greeted(async_imap::Client::new(Stream::Plain(tcp))).await?,
        TlsMode::Starttls => {
            let mut plain = greeted(async_imap::Client::new(Stream::Plain(tcp))).await?;
            plain.run_command_and_check_ok("STARTTLS", None).await.context("STARTTLS")?;
            let Stream::Plain(tcp) = plain.into_inner() else { unreachable!() };
            // No second greeting after STARTTLS.
            async_imap::Client::new(tls_wrap(tcp, &s.host, insecure).await?)
        }
    };
    client.login(&cfg.username, password).await.map_err(|(e, _)| anyhow!("login failed: {e}"))
}

async fn greeted(mut c: async_imap::Client<Stream>) -> Result<async_imap::Client<Stream>> {
    c.read_response().await?.ok_or_else(|| anyhow!("server closed the connection"))?;
    Ok(c)
}
