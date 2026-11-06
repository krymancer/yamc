//! Turning email bodies into something the native HTML view lays out well, and images.
//!
//! The renderer is native (no browser engine), so mail is always drawn in the theme's colours:
//! nothing to flash white. Layout tables are flattened, styles dropped, remote images blocked
//! until asked for.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use base64::Engine;
use gpui_kit::{Image, ImageFormat, ImageSource, SharedUri};
use regex::Regex;

static DROP_BLOCKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<(style|script|head|title|xml|noscript)\b[^>]*>.*?</\s*(style|script|head|title|xml|noscript)\s*>").unwrap());
static COMMENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static TABLE_TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<(/?)(table|tbody|thead|tfoot|tr|td|th|center)\b[^>]*>").unwrap());
static SPACER_IMG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)<img\b[^>]*\b(width|height)\s*=\s*["']?[01]["'\s>/][^>]*>"#).unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\bhttps?://[^\s<>"]+"#).unwrap());
static REMOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)<img[^>]+src\s*=\s*["']?\s*https?:"#).unwrap());

pub fn has_remote_images(html: &str) -> bool {
    REMOTE.is_match(html)
}

/// Simplify marketing HTML into flowing blocks.
pub fn prepare_html(html: &str) -> String {
    let s = COMMENTS.replace_all(html, "");
    let s = DROP_BLOCKS.replace_all(&s, "");
    // Tracking pixels and spacers.
    let s = SPACER_IMG.replace_all(&s, "");
    // Mail uses tables for layout almost always; data tables (with <th>) keep their structure.
    if s.to_ascii_lowercase().contains("<th") {
        return s.into_owned();
    }
    TABLE_TAGS.replace_all(&s, |c: &regex::Captures| format!("<{}div>", &c[1])).into_owned()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Plain text as HTML: paragraphs, line breaks, links, and quoted blocks set apart.
pub fn text_to_html(text: &str) -> String {
    let mut out = String::new();
    let mut quote: Vec<String> = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let link = |l: &str| URL.replace_all(&escape(l), |c: &regex::Captures| format!("<a href=\"{0}\">{0}</a>", &c[0])).into_owned();
    let flush_para = |para: &mut Vec<String>, out: &mut String| {
        if !para.is_empty() {
            out.push_str("<p>");
            out.push_str(&para.join("<br>"));
            out.push_str("</p>");
            para.clear();
        }
    };
    for line in text.replace("\r\n", "\n").lines() {
        let t = line.trim_start();
        if t.starts_with('>') {
            flush_para(&mut para, &mut out);
            // Nested levels (`> > x`, `>> x`) collapse into one quote block.
            let q = t.trim_start_matches(|c: char| c == '>' || c == ' ');
            quote.push(link(q));
            continue;
        }
        if !quote.is_empty() {
            out.push_str("<blockquote><p>");
            out.push_str(&quote.join("<br>"));
            out.push_str("</p></blockquote>");
            quote.clear();
        }
        if t.is_empty() {
            flush_para(&mut para, &mut out);
        } else {
            para.push(link(line));
        }
    }
    flush_para(&mut para, &mut out);
    if !quote.is_empty() {
        out.push_str("<blockquote><p>");
        out.push_str(&quote.join("<br>"));
        out.push_str("</p></blockquote>");
    }
    out
}

fn format_for(mime: &str) -> Option<ImageFormat> {
    Some(match mime.to_ascii_lowercase().as_str() {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/svg+xml" => ImageFormat::Svg,
        "image/bmp" => ImageFormat::Bmp,
        "image/tiff" => ImageFormat::Tiff,
        _ => return None,
    })
}

fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    Some(match bytes {
        [0x89, b'P', b'N', b'G', ..] => ImageFormat::Png,
        [0xff, 0xd8, ..] => ImageFormat::Jpeg,
        [b'G', b'I', b'F', ..] => ImageFormat::Gif,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ImageFormat::Webp,
        [b'B', b'M', ..] => ImageFormat::Bmp,
        _ if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") => ImageFormat::Svg,
        _ => return None,
    })
}

// A transparent 1×1 GIF.
const BLANK: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

fn blank() -> ImageSource {
    static B: LazyLock<Arc<Image>> = LazyLock::new(|| Arc::new(Image::from_bytes(ImageFormat::Gif, BLANK.to_vec())));
    ImageSource::Image(B.clone())
}

fn decode_data_url(uri: &str) -> Option<Arc<Image>> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("");
    let bytes = if meta.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?
    } else {
        data.as_bytes().to_vec()
    };
    let format = format_for(mime).or_else(|| sniff(&bytes))?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

/// Remote images that have been fetched (shared by every message view).
#[derive(Clone, Default)]
pub struct Images(Arc<Mutex<HashMap<String, Option<Arc<Image>>>>>);

impl Images {
    /// Image source for one message. Remote images resolve only when `remote` is on and fetched.
    pub fn resolver(&self, remote: bool) -> impl Fn(&SharedUri) -> ImageSource + Send + Sync + 'static {
        let cache = self.0.clone();
        let inline: Arc<Mutex<HashMap<String, Arc<Image>>>> = Arc::default();
        move |uri: &SharedUri| {
            let u = uri.as_ref();
            if u.starts_with("data:") {
                let mut inl = inline.lock().unwrap();
                if let Some(img) = inl.get(u) {
                    return ImageSource::Image(img.clone());
                }
                return match decode_data_url(u) {
                    Some(img) => {
                        inl.insert(u.to_string(), img.clone());
                        ImageSource::Image(img)
                    }
                    None => blank(),
                };
            }
            if remote && let Some(Some(img)) = cache.lock().unwrap().get(u) {
                return ImageSource::Image(img.clone());
            }
            blank()
        }
    }

    /// URLs of remote images in `html` that aren't fetched yet.
    pub fn missing(&self, html: &str) -> Vec<String> {
        static SRC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)<img[^>]+src\s*=\s*["']?(https?://[^"'\s>]+)"#).unwrap());
        let cache = self.0.lock().unwrap();
        let mut out: Vec<String> = SRC
            .captures_iter(html)
            .map(|c| c[1].replace("&amp;", "&"))
            .filter(|u| !cache.contains_key(u))
            .collect();
        out.dedup();
        out
    }

    pub async fn fetch(&self, urls: Vec<String>) {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("yamc")
            .build()
            .unwrap_or_default();
        let jobs = urls.into_iter().map(|u| {
            let client = client.clone();
            async move {
                let img = async {
                    let r = client.get(&u).send().await.ok()?.error_for_status().ok()?;
                    let mime = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    let bytes = r.bytes().await.ok()?.to_vec();
                    let format = format_for(mime.split(';').next().unwrap_or("")).or_else(|| sniff(&bytes))?;
                    Some(Arc::new(Image::from_bytes(format, bytes)))
                }
                .await;
                (u, img)
            }
        });
        for (u, img) in futures::future::join_all(jobs).await {
            self.0.lock().unwrap().insert(u, img);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_layout() {
        let h = r#"<html><head><style>p{color:red}</style></head><body><table width="600"><tr><td>Hi <img src="https://t.co/p.gif" width="1" height="1"></td></tr></table></body></html>"#;
        let p = prepare_html(h);
        assert!(!p.contains("<table") && !p.contains("color:red") && !p.contains("p.gif"));
        assert!(p.contains("<div>Hi"));
        assert!(prepare_html("<table><tr><th>A</th></tr></table>").contains("<table"));
    }

    #[test]
    fn plain_text() {
        let h = text_to_html("Hi <you>\nsee https://x.com/a?b=1&c\n\n> quoted\n> more\nbye");
        assert_eq!(
            h,
            "<p>Hi &lt;you&gt;<br>see <a href=\"https://x.com/a?b=1&amp;c\">https://x.com/a?b=1&amp;c</a></p><blockquote><p>quoted<br>more</p></blockquote><p>bye</p>"
        );
    }

    #[test]
    fn remote_detection() {
        assert!(has_remote_images(r#"<img src="https://x/y.png">"#));
        assert!(!has_remote_images(r#"<img src="data:image/png;base64,AA">"#));
        let i = Images::default();
        assert_eq!(i.missing(r#"<img src="https://a/b.png?x=1&amp;y=2"><img src='https://a/b.png?x=1&amp;y=2'>"#), ["https://a/b.png?x=1&y=2"]);
    }
}
