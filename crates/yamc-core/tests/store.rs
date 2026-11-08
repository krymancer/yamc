//! Store behaviour: threading, unread-first order, search, and listing speed at 100k messages.

use std::time::Instant;

use yamc_core::secrets::SecretBox;
use yamc_core::store::{Body, NewMessage, Store};
use yamc_core::types::*;

fn store() -> (Store, std::path::PathBuf) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("yamc-store-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let s = Store::open(&dir.join("t.db"), SecretBox::from_key(&[7; 32], "test")).unwrap();
    (s, dir)
}

fn account(s: &Store) -> (i64, i64) {
    let cfg = yamc_core::presets::preset_config(AccountKind::Imap, "me@x.test");
    let a = s
        .add_account(&AccountInput { name: "Me".into(), sender_name: String::new(), email: "me@x.test".into(), kind: AccountKind::Imap, config: cfg, password: "pw".into() })
        .unwrap();
    let inbox = s.upsert_mailbox(a, "INBOX", "Inbox", Some("/"), Some("\\Inbox"), 0, true).unwrap();
    (a, inbox)
}

fn msg(a: i64, mb: i64, uid: u32, id: &str, reply: Option<&str>, date: i64, seen: bool) -> NewMessage {
    NewMessage {
        account_id: a,
        mailbox_id: mb,
        uid,
        message_id: Some(id.into()),
        in_reply_to: reply.map(String::from),
        references: reply.map(|r| vec![r.to_string()]).unwrap_or_default(),
        subject: format!("subject {uid}"),
        from: Some(Address { name: Some(format!("Person {}", uid % 50)), address: format!("p{}@x.test", uid % 50) }),
        date,
        seen,
        ..Default::default()
    }
}

#[test]
fn threads_unread_first_and_search() {
    let (s, dir) = store();
    let (a, inbox) = account(&s);
    s.insert_messages(&[
        msg(a, inbox, 1, "<a@x>", None, 1000, true),
        // A reply that arrives before its parent still joins the thread (References has the chain).
        NewMessage { references: vec!["<a@x>".into(), "<b@x>".into()], ..msg(a, inbox, 3, "<c@x>", Some("<b@x>"), 3000, true) },
        msg(a, inbox, 2, "<b@x>", Some("<a@x>"), 2000, true),
        msg(a, inbox, 4, "<d@x>", None, 500, false),
    ])
    .unwrap();
    let rows = s.list_threads(&View::Inbox, 0, 100);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].unread, "unread first even though it's older");
    assert_eq!(rows[1].count, 3);
    assert_eq!(s.get_password(a).unwrap(), "pw");
    s.rename_account(a, "Work", "  ").unwrap();
    assert_eq!(s.account(a).unwrap().name, "Work");
    assert_eq!(s.account(a).unwrap().sender_name, None, "blank sender name means infer it");
    s.rename_account(a, "Work", "Júnior").unwrap();
    assert_eq!(s.account(a).unwrap().sender_name.as_deref(), Some("Júnior"));
    assert_eq!(s.get_password(a).unwrap(), "pw", "renaming keeps the sealed password");

    let id = s.get_thread_messages(a, &rows[1].thread_id)[0].id;
    s.save_body(id, &Body { text: Some("the ramen place on fifth".into()), snippet: "the ramen".into(), ..Default::default() }).unwrap();
    let hits = s.list_threads(&View::Search("rame".into()), 0, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].thread_id, rows[1].thread_id);
    assert!(s.list_threads(&View::Search("\"unbalanced".into()), 0, 10).is_empty());
    // Field filters.
    assert_eq!(s.list_threads(&View::Search("from:p4".into()), 0, 10).len(), 1);
    assert_eq!(s.list_threads(&View::Search("from:person".into()), 0, 10).len(), 2);
    assert!(s.list_threads(&View::Search("subject:rame".into()), 0, 10).is_empty());
    assert_eq!(s.list_threads(&View::Search("body:rame from:person".into()), 0, 10).len(), 1);
    assert_eq!(s.list_threads(&View::Search("unknown:rame".into()), 0, 10).len(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn drafts_keep_attachments() {
    let (s, dir) = store();
    let (a, _) = account(&s);
    let id = s
        .save_draft(&DraftInput { account_id: a, subject: "files".into(), attachments: vec!["/tmp/a.pdf".into(), "/tmp/b.png".into()], ..Default::default() })
        .unwrap();
    let d = s.get_draft(id).unwrap();
    assert_eq!(d.attachments, ["/tmp/a.pdf", "/tmp/b.png"]);
    s.save_draft(&DraftInput { id: Some(id), attachments: vec![], ..DraftInput::from(&d) }).unwrap();
    assert!(s.get_draft(id).unwrap().attachments.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sender_name_comes_from_sent_mail() {
    let (s, dir) = store();
    let (a, inbox) = account(&s);
    assert_eq!(s.inferred_sender_name(a, "me@x.test"), None);
    let me = |name: &str| Some(Address { name: Some(name.into()), address: "ME@x.test".into() });
    s.insert_messages(&[
        NewMessage { from: me("Júnior Nascimento"), ..msg(a, inbox, 1, "<1@x>", None, 1000, true) },
        NewMessage { from: me("Júnior Nascimento"), ..msg(a, inbox, 2, "<2@x>", None, 2000, true) },
        // One message sent under a label by mistake doesn't take over.
        NewMessage { from: me("Gmail"), ..msg(a, inbox, 3, "<3@x>", None, 3000, true) },
        msg(a, inbox, 4, "<4@x>", None, 4000, true),
    ])
    .unwrap();
    assert_eq!(s.inferred_sender_name(a, "me@x.test").as_deref(), Some("Júnior Nascimento"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sent_view_shows_recipients() {
    let (s, dir) = store();
    let (a, inbox) = account(&s);
    let sent = s.upsert_mailbox(a, "Sent", "Sent", Some("/"), Some("\\Sent"), 0, true).unwrap();
    s.insert_messages(&[
        msg(a, inbox, 1, "<in@x>", None, 1000, false),
        NewMessage {
            to: vec![Address { name: Some("Ana".into()), address: "ana@x.test".into() }, Address { name: None, address: "bo@x.test".into() }],
            ..msg(a, sent, 1, "<out@x>", None, 2000, true)
        },
    ])
    .unwrap();
    let rows = s.list_threads(&View::Sent, 0, 10);
    assert_eq!(rows.len(), 1, "only Sent-folder conversations");
    assert_eq!(rows[0].from, "To: Ana, bo@x.test");
    assert_eq!(s.list_threads(&View::Inbox, 0, 10).len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lists_100k_messages_fast() {
    let (s, dir) = store();
    let (a, inbox) = account(&s);
    let batch: Vec<NewMessage> =
        (1..=100_000u32).map(|i| msg(a, inbox, i, &format!("<{i}@x>"), None, i as i64 * 1000, i % 7 != 0)).collect();
    let t = Instant::now();
    for chunk in batch.chunks(250) {
        s.insert_messages(chunk).unwrap();
    }
    eprintln!("insert 100k headers: {:?}", t.elapsed());
    let t = Instant::now();
    let rows = s.list_threads(&View::Inbox, 0, 300);
    let first = t.elapsed();
    let t = Instant::now();
    let _ = s.list_threads(&View::Inbox, 0, 300);
    let again = t.elapsed();
    eprintln!("list first 300 of 100k: {first:?} (warm: {again:?})");
    assert_eq!(rows.len(), 300);
    assert!(rows[0].unread);
    assert!(again.as_millis() < 200, "listing took {again:?}");
    // Search runs on every keystroke, so even a one-letter prefix matching everything must be quick.
    let t = Instant::now();
    let hits = s.list_threads(&View::Search("s".into()), 0, 100);
    eprintln!("search 's' over 100k: {:?}", t.elapsed());
    assert_eq!(hits.len(), 100);
    let _ = std::fs::remove_dir_all(dir);
}
