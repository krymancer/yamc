//! Outgoing mail over SMTP (lettre + rustls).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use lettre::message::{Mailbox, MessageBuilder, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::net::insecure_ok;
use crate::types::{AccountConfig, TlsMode};

pub fn transport(cfg: &AccountConfig, password: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let s = &cfg.smtp;
    let params = || {
        TlsParameters::builder(s.host.clone()).dangerous_accept_invalid_certs(insecure_ok(cfg, s)).build_rustls()
    };
    let builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&s.host).port(s.port);
    let builder = match s.tls {
        TlsMode::Tls => builder.tls(Tls::Wrapper(params()?)),
        TlsMode::Starttls => builder.tls(Tls::Required(params()?)),
        TlsMode::None => builder.tls(Tls::None),
    };
    Ok(builder
        .credentials(Credentials::new(cfg.username.clone(), password.to_string()))
        .timeout(Some(Duration::from_secs(30)))
        .build())
}

pub async fn verify(cfg: &AccountConfig, password: &str) -> Result<()> {
    let ok = transport(cfg, password)?.test_connection().await?;
    if !ok {
        return Err(anyhow!("server refused the connection"));
    }
    Ok(())
}

pub struct Outgoing<'a> {
    pub from_name: &'a str,
    pub from_addr: &'a str,
    pub to: &'a [String],
    pub cc: &'a [String],
    pub subject: &'a str,
    pub body: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
}

fn mailbox(s: &str) -> Result<Mailbox> {
    s.parse::<Mailbox>().with_context(|| format!("bad address: {s}"))
}

pub fn build(o: &Outgoing<'_>) -> Result<Message> {
    let from = Mailbox::new(Some(o.from_name.to_string()).filter(|n| !n.is_empty()), o.from_addr.parse()?);
    let mut b: MessageBuilder = Message::builder().from(from).subject(o.subject);
    for t in o.to {
        b = b.to(mailbox(t)?);
    }
    for c in o.cc {
        b = b.cc(mailbox(c)?);
    }
    if let Some(r) = o.in_reply_to {
        b = b.in_reply_to(r.to_string());
    }
    if !o.references.is_empty() {
        b = b.references(o.references.join(" "));
    }
    Ok(b.header(ContentType::TEXT_PLAIN).body(o.body.to_string())?)
}

pub async fn send(cfg: &AccountConfig, password: &str, msg: Message) -> Result<()> {
    transport(cfg, password)?.send(msg).await?;
    Ok(())
}
