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
        .add_account(&AccountInput { name: "Me".into(), email: "me@x.test".into(), kind: AccountKind::Imap, config: cfg, password: "pw".into() })
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

    let id = s.get_thread_messages(a, &rows[1].thread_id)[0].id;
    s.save_body(id, &Body { text: Some("the ramen place on fifth".into()), snippet: "the ramen".into(), ..Default::default() }).unwrap();
    let hits = s.list_threads(&View::Search("rame".into()), 0, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].thread_id, rows[1].thread_id);
    assert!(s.list_threads(&View::Search("\"unbalanced".into()), 0, 10).is_empty());
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
    let _ = std::fs::remove_dir_all(dir);
}
