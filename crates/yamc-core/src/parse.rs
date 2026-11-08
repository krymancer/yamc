//! MIME parsing: header blocks from sync, full sources for bodies.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mail_parser::{AddressList, MessageParser, PartKind};

use crate::store::Body;
use crate::types::{Address, Attachment};

const INLINE_MAX: usize = 2 * 1024 * 1024;

/// The header fields sync needs, parsed from `BODY[HEADER.FIELDS (...)]`.
#[derive(Debug, Default, Clone)]
pub struct Headers {
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub subject: String,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Milliseconds since the epoch.
    pub date: Option<i64>,
}

fn addresses(list: Option<AddressList<'_>>) -> Vec<Address> {
    let Some(list) = list else { return Vec::new() };
    list.mailboxes()
        .filter_map(|m| {
            Some(Address {
                name: m.name().map(str::trim).filter(|n| !n.is_empty()).map(String::from),
                address: m.address()?.trim().to_string(),
            })
        })
        .filter(|a| !a.address.is_empty())
        .collect()
}

fn angle(id: &str) -> String {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    format!("<{id}>")
}

pub fn parse_headers(raw: &[u8]) -> Headers {
    let Some(msg) = MessageParser::default().parse(raw) else { return Headers::default() };
    let m = msg.root();
    Headers {
        message_id: m.message_id().filter(|s| !s.trim().is_empty()).map(angle),
        in_reply_to: m.in_reply_to().and_then(|l| l.first().map(angle)),
        references: m.references().map(|l| l.iter().map(angle).collect()).unwrap_or_default(),
        subject: m.subject().unwrap_or("").trim().to_string(),
        from: addresses(m.from()).into_iter().next(),
        to: addresses(m.to()),
        cc: addresses(m.cc()),
        date: m.date().map(|d| d.to_timestamp() * 1000),
    }
}

pub fn html_to_text(html: &str) -> String {
    mail_parser::html_to_text(html)
}

/// A one-line preview that skips quoted replies.
pub fn make_snippet(text: &str) -> String {
    let mut lines = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        // The reply's own text ends where the quoted message begins.
        if (t.starts_with("On ") && t.ends_with("wrote:")) || t.starts_with("-----Original Message") {
            break;
        }
        if !t.starts_with('>') {
            lines.push(l);
        }
    }
    let joined = lines.join(" ");
    let words = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    words.chars().take(200).collect()
}

pub fn parse_source(raw: &[u8]) -> Option<Body> {
    parse_full(raw, None).map(|(body, _)| body)
}

/// The name and bytes of the `index`th entry of `Body::attachments` for this source.
pub fn attachment_data(raw: &[u8], index: usize) -> Option<(String, Vec<u8>)> {
    parse_full(raw, Some(index))?.1
}

/// Parse a full source; `want` also pulls out that attachment's bytes. Both callers share
/// this walk so the attachment indexes always agree.
fn parse_full(raw: &[u8], want: Option<usize>) -> Option<(Body, Option<(String, Vec<u8>)>)> {
    let msg = MessageParser::default().parse(raw)?;
    let m = msg.root();

    let html_parts: Vec<String> = m
        .html_body()
        .filter(|p| matches!(p.kind(), PartKind::Html))
        .filter_map(|p| p.text().map(|s| s.into_owned()))
        .collect();
    let mut html = (!html_parts.is_empty()).then(|| html_parts.join("\n"));

    let text_parts: Vec<String> = m
        .text_body()
        .filter(|p| matches!(p.kind(), PartKind::Text))
        .filter_map(|p| p.text().map(|s| s.into_owned()))
        .collect();
    let text = if !text_parts.is_empty() {
        Some(text_parts.join("\n"))
    } else {
        html.as_deref().map(html_to_text)
    };

    let mut attachments = Vec::new();
    let mut wanted = None;
    for a in m.attachments() {
        let mime = a
            .content_type()
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
            .unwrap_or_else(|| "application/octet-stream".into());
        let data = a.decoded();
        if let (Some(h), Some(cid)) = (html.as_mut(), a.content_id()) {
            let cid = cid.trim().trim_start_matches('<').trim_end_matches('>');
            let needle = format!("cid:{cid}");
            if !cid.is_empty() && h.contains(&needle) && data.len() <= INLINE_MAX {
                // Inline images become data URLs so nothing has to be fetched.
                *h = h.replace(&needle, &format!("data:{mime};base64,{}", STANDARD.encode(&data)));
                continue;
            }
        }
        if matches!(a.kind(), PartKind::Message(_)) || a.is_attachment() || a.attachment_name().is_some() || !a.is_text() {
            let filename = a.attachment_name().unwrap_or("attachment").to_string();
            if want == Some(attachments.len()) {
                wanted = Some((filename.clone(), data.to_vec()));
            }
            attachments.push(Attachment {
                filename,
                mime_type: mime,
                size: data.len() as u64,
            });
        }
    }

    let snippet = make_snippet(text.as_deref().unwrap_or(""));
    Some((Body { html, text, attachments, snippet }, wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        let raw = b"From: =?UTF-8?Q?Jo=C3=A3o?= <joao@example.com>\r\nTo: a@x.com, \"B\" <b@x.com>\r\nSubject: Hi\r\nMessage-ID: <1@x>\r\nIn-Reply-To: <0@x>\r\nReferences: <r@x> <0@x>\r\nDate: Sat, 20 Nov 2021 14:22:01 -0800\r\n\r\n";
        let h = parse_headers(raw);
        assert_eq!(h.from.unwrap().name.as_deref(), Some("João"));
        assert_eq!(h.to.len(), 2);
        assert_eq!(h.message_id.as_deref(), Some("<1@x>"));
        assert_eq!(h.in_reply_to.as_deref(), Some("<0@x>"));
        assert_eq!(h.references, vec!["<r@x>", "<0@x>"]);
        assert_eq!(h.date, Some(1637446921000));
    }

    #[test]
    fn snippet_skips_quotes() {
        assert_eq!(make_snippet("Sounds good\n\n> old stuff\n>> older\nthanks"), "Sounds good thanks");
        assert_eq!(make_snippet("Yes!\n\nOn Thu, Nov 5, 2026 at 20:45, Carol <c@x> wrote:\n> hi"), "Yes!");
    }

    #[test]
    fn attachment_bytes_by_index() {
        let raw = concat!(
            "From: a@x.com\r\nSubject: s\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n",
            "--b\r\nContent-Type: text/plain\r\n\r\nsee attached\r\n",
            "--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQ=\r\n",
            "--b\r\nContent-Type: text/csv\r\nContent-Disposition: attachment; filename=\"b.csv\"\r\n\r\nx,y\r\n",
            "--b--\r\n"
        );
        let b = parse_source(raw.as_bytes()).unwrap();
        let names: Vec<_> = b.attachments.iter().map(|a| a.filename.as_str()).collect();
        assert_eq!(names, ["a.pdf", "b.csv"]);
        assert_eq!(attachment_data(raw.as_bytes(), 0).unwrap(), ("a.pdf".to_string(), b"%PDF-1.4".to_vec()));
        assert_eq!(attachment_data(raw.as_bytes(), 1).unwrap().0, "b.csv");
        assert!(attachment_data(raw.as_bytes(), 2).is_none());
    }

    #[test]
    fn body_with_inline_image() {
        let raw = concat!(
            "From: a@x.com\r\nSubject: s\r\nContent-Type: multipart/related; boundary=b\r\n\r\n",
            "--b\r\nContent-Type: text/html\r\n\r\n<p>hi <img src=\"cid:img1\"></p>\r\n",
            "--b\r\nContent-Type: image/gif\r\nContent-ID: <img1>\r\nContent-Transfer-Encoding: base64\r\n\r\nR0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7\r\n",
            "--b--\r\n"
        );
        let b = parse_source(raw.as_bytes()).unwrap();
        assert!(b.html.as_deref().unwrap().contains("data:image/gif;base64,"));
        assert!(b.attachments.is_empty());
        assert_eq!(b.snippet, "hi");
    }
}
