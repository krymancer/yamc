//! End-to-end against a real IMAP/SMTP server. Needs GreenMail:
//!   docker run -d --rm --name yamc-greenmail -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled' \
//!     -p 3025:3025 -p 3143:3143 greenmail/standalone
//! then: YAMC_IT=1 cargo test -p yamc-core --test greenmail

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::TryStreamExt;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use yamc_core::core::Core;
use yamc_core::mcp::McpServer;
use yamc_core::net::imap_connect;
use yamc_core::types::*;

fn config(user: &str) -> AccountConfig {
    AccountConfig {
        imap: ServerConfig { host: "127.0.0.1".into(), port: 3143, tls: TlsMode::None },
        smtp: ServerConfig { host: "127.0.0.1".into(), port: 3025, tls: TlsMode::None },
        username: user.into(),
        allow_self_signed: false,
        server_saves_sent: false,
    }
}

async fn until<T: std::fmt::Debug, F: Future<Output = T>>(mut f: impl FnMut() -> F, ok: impl Fn(&T) -> bool, secs: u64) -> T {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let v = f().await;
        if ok(&v) {
            return v;
        }
        if Instant::now() > end {
            panic!("timed out; last value: {v:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn deliver(raw: &str) {
    let smtp = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1").port(3025).build();
    let env = lettre::address::Envelope::new(
        Some("sender@x.test".parse().unwrap()),
        vec![raw.lines().find_map(|l| l.strip_prefix("To: ")).unwrap().parse().unwrap()],
    )
    .unwrap();
    smtp.send_raw(&env, raw.replace('\n', "\r\n").as_bytes()).await.unwrap();
}

fn inbox(core: &Core) -> Vec<ThreadRow> {
    core.list_threads(&View::Inbox, 0, 100)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn core_against_greenmail() {
    if std::env::var_os("YAMC_IT").is_none() {
        eprintln!("skipped: set YAMC_IT=1 with GreenMail running");
        return;
    }
    // SAFETY: set before any other thread reads the environment.
    unsafe { std::env::set_var("YAMC_NO_KEYCHAIN", "1") };
    let stamp = yamc_core::store::now_ms();
    let user = format!("it{stamp}@localhost");
    let cfg = config(&user);

    // Make the mailbox exist with an Archive folder, plus mail waiting before first sync.
    let mut s = imap_connect(&cfg, "pw").await.unwrap();
    let _ = s.create("Archive").await;
    s.logout().await.unwrap();
    let date = |ago: i64| (chrono::Utc::now() - chrono::Duration::minutes(ago)).to_rfc2822();
    deliver(&format!("Date: {}\nFrom: Bob <bob@x.test>\nTo: {user}\nSubject: Lunch?\nMessage-ID: <lunch{stamp}@x.test>\n\nLunch tomorrow?\n", date(3))).await;
    deliver(&format!(
        "Date: {}\nFrom: Carol <carol@x.test>\nTo: {user}\nSubject: Re: Lunch?\nIn-Reply-To: <lunch{stamp}@x.test>\nReferences: <lunch{stamp}@x.test>\n\nI am in.\n> Lunch tomorrow?\n",
        date(2)
    ))
    .await;
    deliver(&format!(
        "Date: {}\nFrom: News <news@x.test>\nTo: {user}\nSubject: Weekly digest\nContent-Type: text/html\n\n<body style=\"background:#fff\"><p style=\"color:#000\">Hello <img src=\"https://example.com/t.png\"></p></body>\n",
        date(1)
    ))
    .await;

    let dir = std::env::temp_dir().join(format!("yamc-it-{stamp}"));
    let core = Core::open(&dir, tokio::runtime::Handle::current()).unwrap();
    let account_id = core
        .add_account(AccountInput { name: "Me".into(), sender_name: String::new(), email: user.clone(), kind: AccountKind::Imap, config: cfg.clone(), password: "pw".into() })
        .await
        .unwrap();

    // syncs the inbox into conversations, unread first
    let rows = until(|| async { inbox(&core) }, |r| r.len() == 2, 15).await;
    assert!(rows.iter().all(|r| r.unread));
    let lunch = rows.iter().find(|r| r.subject.contains("Lunch")).unwrap().clone();
    assert_eq!(lunch.count, 2);
    assert_eq!(rows[0].subject, "Weekly digest");

    // downloads bodies and fills snippets
    let rows = until(|| async { inbox(&core) }, |r| r.iter().all(|x| !x.snippet.is_empty()), 15).await;
    assert!(rows.iter().find(|r| r.subject == "Weekly digest").unwrap().snippet.contains("Hello"));
    let t = core.get_thread(account_id, &lunch.thread_id);
    let m = core.load_message(t.messages[0].id).await.unwrap();
    assert!(m.text.unwrap().contains("Lunch tomorrow?"));

    // marks read on the server
    core.mark_thread(account_id, &lunch.thread_id, true).unwrap();
    assert!(!inbox(&core).iter().find(|r| r.key() == lunch.key()).unwrap().unread);
    let end = Instant::now() + Duration::from_secs(15);
    let mut s = imap_connect(&cfg, "pw").await.unwrap();
    loop {
        s.select("INBOX").await.unwrap();
        if s.uid_search("SEEN").await.unwrap().len() == 2 {
            break;
        }
        assert!(Instant::now() < end, "server never saw the read flags");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    s.logout().await.unwrap();

    // searches
    let found: Vec<_> = core.list_threads(&View::Search("digest".into()), 0, 10).into_iter().map(|r| r.subject).collect();
    assert_eq!(found, vec!["Weekly digest"]);

    // sends mail that comes back via IDLE
    let t = core.get_thread(account_id, &lunch.thread_id);
    let d = core.reply_draft(t.messages[1].id, false).unwrap();
    assert_eq!(d.subject, "Re: Lunch?");
    core.send(&DraftInput { to: user.clone(), body: format!("See you there{}", d.body), ..d }).await.unwrap();
    let again = until(
        || async { inbox(&core).into_iter().find(|r| r.subject.contains("Lunch")) },
        |r| r.as_ref().is_some_and(|r| r.count == 3),
        20,
    )
    .await
    .unwrap();
    assert!(again.unread);

    // archives into the Archive folder
    let digest = inbox(&core).into_iter().find(|r| r.subject == "Weekly digest").unwrap();
    core.archive_thread(account_id, &digest.thread_id).unwrap();
    assert!(!inbox(&core).iter().any(|r| r.key() == digest.key()));
    let archive = core.list_mailboxes().into_iter().find(|m| m.special_use.as_deref() == Some("\\Archive"));
    let archive = archive.or_else(|| core.list_mailboxes().into_iter().find(|m| m.path == "Archive")).unwrap();
    until(|| async { core.list_threads(&View::Mailbox(archive.id), 0, 10) }, |r| r.len() == 1, 20).await;
    let mut s = imap_connect(&cfg, "pw").await.unwrap();
    s.select("Archive").await.unwrap();
    let in_archive: Vec<_> = s.uid_fetch("1:*", "(UID)").await.unwrap().try_collect().await.unwrap();
    assert_eq!(in_archive.len(), 1);
    s.logout().await.unwrap();

    // serves MCP tools over HTTP with a token
    let port = 47000 + (stamp % 1000) as u16;
    let server = McpServer::start(core.clone(), port, "secret".into()).unwrap();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let bad = reqwest::Client::new().post(&url).body("{}").send().await.unwrap();
    assert_eq!(bad.status(), 401);
    let transport = StreamableHttpClientTransport::from_config(StreamableHttpClientTransportConfig::with_uri(url.clone()).auth_header("secret"));
    let client = ().serve(transport).await.unwrap();
    let mut tools: Vec<String> = client.list_all_tools().await.unwrap().into_iter().map(|t| t.name.to_string()).collect();
    tools.sort();
    assert_eq!(tools, ["archive", "create_draft", "get_thread", "list_accounts", "list_drafts", "list_inbox", "mark_read", "search"]);
    let call = |name: &'static str, args: Value| {
        let client = &client;
        async move {
            let r = client
                .call_tool(CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone()))
                .await
                .unwrap();
            let text = r.content[0].as_text().unwrap().text.clone();
            serde_json::from_str::<Value>(&text).unwrap()
        }
    };
    let list = call("list_inbox", json!({})).await;
    let lunch = list.as_array().unwrap().iter().find(|t| t["subject"].as_str().unwrap().contains("Lunch")).unwrap().clone();
    let thread = call("get_thread", json!({ "account_id": lunch["account_id"], "thread_id": lunch["thread_id"] })).await;
    assert!(thread["messages"][0]["body"].as_str().unwrap().contains("Lunch tomorrow?"));
    let last = thread["messages"].as_array().unwrap().last().unwrap()["message_id"].clone();
    let draft = call("create_draft", json!({ "reply_to_message_id": last, "body": "Sounds good" })).await;
    let saved = core.get_draft(draft["draft_id"].as_i64().unwrap()).unwrap();
    assert_eq!(saved.created_by, "agent");
    assert_eq!(saved.subject, "Re: Lunch?");
    assert!(saved.body.starts_with("Sounds good"));
    drop(client);
    server.stop();

    core.stop();
    let _ = Arc::strong_count(&core);
    let _ = std::fs::remove_dir_all(dir);
}
