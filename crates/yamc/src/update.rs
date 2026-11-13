//! Self-update from GitHub releases. The installed app checks shortly after launch and every
//! few hours; a newer release is downloaded and swapped into place in the background, then the
//! update button at the bottom of the sidebar offers a restart (hover it for the release notes).
//! Builds run from `target/` (development) never update themselves.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::*;
use tokio::runtime::Handle;

const REPO: &str = "krymancer/yamc";
const ASSET: &str = "yamc-macos.zip";
const BUNDLE_ID: &str = "dev.yamc.app";
/// SHA-1 of the release signing certificate ("yamc release signing", self-made, kept in the
/// repo's Actions secrets). Updates signed with anything else are refused.
const SIGNING_CERT: &str = "45E72C5F7F55C6F323D6C533AF45C3BCF0F7D773";
const FIRST_CHECK: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    /// Installed on disk; takes effect on restart.
    Ready(String),
    Failed(String),
}

/// What a newer release says about itself, shown when you hover the update button.
#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    pub version: String,
    /// Unix milliseconds, if GitHub gave a date.
    pub published_ms: Option<i64>,
    pub notes: Vec<Note>,
    /// The release page on GitHub.
    pub page: String,
}

/// One bullet of release notes: `- **Title.** Text`, or just `- Text`.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub title: Option<String>,
    pub text: String,
}

pub struct Updater {
    rt: Handle,
    /// The running `.app`, or `None` for a development build.
    bundle: Option<PathBuf>,
    pub status: Status,
    /// The newer release being downloaded or waiting for a restart.
    pub release: Option<ReleaseInfo>,
    _poll: Task<()>,
}

impl Updater {
    pub fn new(rt: Handle, cx: &mut Context<Self>) -> Self {
        let bundle = running_bundle();
        let poll = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_CHECK).await;
            loop {
                if this.update(cx, |this, cx| this.check(false, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(EVERY).await;
            }
        });
        Self { rt, bundle, status: Status::Idle, release: None, _poll: poll }
    }

    /// Look for a newer release and install it. `manual` reports "up to date" and errors;
    /// background checks stay quiet unless there's something to restart for.
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Checking | Status::Downloading(_) | Status::Ready(_)) {
            return;
        }
        let Some(bundle) = self.bundle.clone() else {
            if manual {
                self.status = Status::Failed("This is a development build; only the installed yamc.app updates itself.".into());
                cx.notify();
            }
            return;
        };
        self.status = Status::Checking;
        cx.notify();
        let task = self.rt.spawn(latest());
        let rt = self.rt.clone();
        cx.spawn(async move |this, cx| {
            let found = task.await.map_err(anyhow::Error::from).and_then(|r| r);
            let release = match found {
                Ok(Some(r)) => r,
                other => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = match other {
                            Err(e) if manual => Status::Failed(format!("{e:#}")),
                            Ok(None) if manual => Status::UpToDate,
                            _ => Status::Idle,
                        };
                        cx.notify();
                    });
                    return;
                }
            };
            let version = release.info.version.clone();
            let _ = this.update(cx, |this, cx| {
                this.release = Some(release.info.clone());
                this.status = Status::Downloading(version.clone());
                cx.notify();
            });
            let r = rt.spawn(async move { install(&release.url, &bundle).await }).await.map_err(anyhow::Error::from).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.status = match r {
                    Ok(()) => Status::Ready(version),
                    Err(e) => Status::Failed(format!("Update to {version} failed: {e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether this copy can replace itself. Builds run from source (`cargo run`) can't, so they
    /// show no update button.
    pub fn can_update(&self) -> bool {
        self.bundle.is_some()
    }

    /// Quit and open the freshly installed app.
    pub fn restart(&self, cx: &mut Context<Self>) {
        let Some(bundle) = &self.bundle else { return };
        // Wait for this process to exit so `open` launches the new copy instead of focusing us.
        let script = format!("while kill -0 {} 2>/dev/null; do sleep 0.2; done; open \"$0\"", std::process::id());
        if Command::new("/bin/sh").arg("-c").arg(script).arg(bundle).spawn().is_ok() {
            cx.quit();
        }
    }
}

/// `/Applications/yamc.app` when running as `…/yamc.app/Contents/MacOS/yamc`.
fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app").then(|| app.to_path_buf())
}

struct Release {
    info: ReleaseInfo,
    /// The zip to download.
    url: String,
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(concat!("yamc/", env!("CARGO_PKG_VERSION"))).timeout(Duration::from_secs(300)).build()?)
}

/// The latest release, if it's newer than this build.
async fn latest() -> Result<Option<Release>> {
    let r = client()?
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None); // No releases yet.
    }
    let json: serde_json::Value = serde_json::from_slice(&r.error_for_status()?.bytes().await?)?;
    let tag = json["tag_name"].as_str().ok_or_else(|| anyhow!("release has no tag"))?;
    let version = tag.trim_start_matches('v').to_string();
    if !newer(&version, VERSION) {
        return Ok(None);
    }
    let url = json["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str() == Some(ASSET))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| anyhow!("release {tag} has no {ASSET}"))?;
    let info = ReleaseInfo {
        version,
        published_ms: json["published_at"].as_str().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok()).map(|d| d.timestamp_millis()),
        notes: parse_notes(json["body"].as_str().unwrap_or_default()),
        page: json["html_url"].as_str().unwrap_or_default().to_string(),
    };
    Ok(Some(Release { info, url: url.to_string() }))
}

/// The bullets of a release's notes; anything that isn't a bullet (headings, links) is dropped.
fn parse_notes(body: &str) -> Vec<Note> {
    body.lines()
        .filter_map(|l| l.trim().strip_prefix("- ").or_else(|| l.trim().strip_prefix("* ")))
        .map(|item| {
            if let Some(rest) = item.strip_prefix("**")
                && let Some((title, text)) = rest.split_once("**")
            {
                return Note { title: Some(title.trim().to_string()), text: text.trim().to_string() };
            }
            Note { title: None, text: item.trim().to_string() }
        })
        .filter(|n| n.title.is_some() || !n.text.is_empty())
        .collect()
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.split(['.', '-', '+']).map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

fn newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// Download the release zip and swap it in for `bundle`. The running process keeps working
/// from the old files it already has open until it restarts.
async fn install(url: &str, bundle: &Path) -> Result<()> {
    let bytes = client()?.get(url).send().await?.error_for_status()?.bytes().await?;
    let parent = bundle.parent().ok_or_else(|| anyhow!("app has no parent folder"))?;
    // Stage next to the app so the final step is a rename on the same volume.
    let stage = parent.join(".yamc-update");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).with_context(|| format!("can't write to {}", parent.display()))?;
    let result = (|| {
        let zip = stage.join(ASSET);
        std::fs::write(&zip, &bytes)?;
        run(Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&zip).arg(&stage))?;
        let new = stage.join("yamc.app");
        check_bundle(&new)?;
        let old = stage.join("old.app");
        std::fs::rename(bundle, &old).with_context(|| format!("can't replace {}", bundle.display()))?;
        if let Err(e) = std::fs::rename(&new, bundle) {
            let _ = std::fs::rename(&old, bundle);
            bail!("can't install the new version: {e}");
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// Refuse anything that isn't an intact yamc bundle.
fn check_bundle(app: &Path) -> Result<()> {
    if !app.join("Contents/MacOS/yamc").is_file() {
        bail!("download doesn't contain yamc.app");
    }
    let plist = app.join("Contents/Info.plist");
    let out = Command::new("/usr/libexec/PlistBuddy").args(["-c", "Print :CFBundleIdentifier"]).arg(&plist).output()?;
    if String::from_utf8_lossy(&out.stdout).trim() != BUNDLE_ID {
        bail!("download isn't yamc ({})", BUNDLE_ID);
    }
    let requirement = format!("=identifier \"{BUNDLE_ID}\" and certificate leaf = H\"{SIGNING_CERT}\"");
    run(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict", "-R"]).arg(requirement).arg(app))
        .context("download isn't signed with yamc's release certificate")
}

fn run(cmd: &mut Command) -> Result<()> {
    let out = cmd.output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Note, newer, parse_notes};

    #[test]
    fn notes() {
        let body = "## What's new\n- **Update button.** Check from the sidebar.\n* Plain bullet\n\n**Full Changelog**: https://x";
        assert_eq!(
            parse_notes(body),
            vec![
                Note { title: Some("Update button.".into()), text: "Check from the sidebar.".into() },
                Note { title: None, text: "Plain bullet".into() },
            ]
        );
    }

    #[test]
    fn versions() {
        assert!(newer("0.2.0", "0.1.9"));
        assert!(newer("1.0.0", "0.9.0"));
        assert!(newer("0.1.10", "0.1.9"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
        assert!(!newer("garbage", "0.1.0"));
    }
}
