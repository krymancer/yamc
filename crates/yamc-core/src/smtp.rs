//! Outgoing mail over SMTP (lettre + rustls).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use lettre::message::{Attachment, Mailbox, MessageBuilder, MultiPart, SinglePart, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::net::{GREETING_TIMEOUT, NoGreeting, insecure_ok};
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
    // lettre's own timeout doesn't cover a server that never sends its greeting.
    let ok = tokio::time::timeout(GREETING_TIMEOUT, transport(cfg, password)?.test_connection())
        .await
        .map_err(|_| NoGreeting(format!("{}:{}", cfg.smtp.host, cfg.smtp.port)))??;
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
    pub attachments: &'a [OutgoingFile],
}

pub struct OutgoingFile {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// MIME type from a file name, for the attachment header.
pub fn content_type_for(filename: &str) -> &'static str {
    let ext = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
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
    if o.attachments.is_empty() {
        return Ok(b.header(ContentType::TEXT_PLAIN).body(o.body.to_string())?);
    }
    let mut parts = MultiPart::mixed().singlepart(SinglePart::plain(o.body.to_string()));
    for f in o.attachments {
        let ct = ContentType::parse(&f.content_type).unwrap_or_else(|_| ContentType::parse("application/octet-stream").expect("mime"));
        parts = parts.singlepart(Attachment::new(f.filename.clone()).body(f.data.clone(), ct));
    }
    Ok(b.multipart(parts)?)
}

pub async fn send(cfg: &AccountConfig, password: &str, msg: Message) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(120), transport(cfg, password)?.send(msg))
        .await
        .map_err(|_| anyhow!("sending timed out"))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outgoing<'a>(files: &'a [OutgoingFile], to: &'a [String]) -> Outgoing<'a> {
        Outgoing {
            from_name: "Me",
            from_addr: "me@x.test",
            to,
            cc: &[],
            subject: "hi",
            body: "see attached",
            in_reply_to: None,
            references: &[],
            attachments: files,
        }
    }

    #[test]
    fn attachments_make_a_multipart_message() {
        let to = vec!["you@x.test".to_string()];
        let plain = String::from_utf8(build(&outgoing(&[], &to)).unwrap().formatted()).unwrap();
        assert!(plain.contains("Content-Type: text/plain"));
        assert!(!plain.contains("multipart"));

        let files = [OutgoingFile { filename: "photo.png".into(), content_type: content_type_for("photo.png").into(), data: vec![137, 80, 78, 71] }];
        let raw = String::from_utf8(build(&outgoing(&files, &to)).unwrap().formatted()).unwrap();
        assert!(raw.contains("multipart/mixed"));
        assert!(raw.contains("see attached"));
        assert!(raw.contains("Content-Type: image/png"));
        assert!(raw.contains("filename=\"photo.png\""));
    }

    #[test]
    fn autocompleted_recipients_parse() {
        for r in ["Júnior Nascimento <a@b.test>", "\"Lucas F. Silva\" <l@b.test>", "\"ACME (support)\" <s@b.test>", "x@b.test"] {
            assert!(mailbox(r).is_ok(), "{r}");
        }
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type_for("a.PDF"), "application/pdf");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
    }
}
