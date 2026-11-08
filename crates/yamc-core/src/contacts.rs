//! Address book for recipient autocomplete, built from mail already synced: everyone you've
//! written to or heard from, best first. People you send to outrank people who send to you.

use std::collections::HashMap;

use crate::types::Address;

/// One sighting of an address: in a message's From, or in To/Cc of mail you sent.
pub struct Seen {
    pub address: String,
    pub name: Option<String>,
    pub date: i64,
    pub sent_to: bool,
}

/// Automated senders nobody writes back to.
fn is_robot(addr: &str) -> bool {
    let local = addr.split('@').next().unwrap_or("");
    ["noreply", "no-reply", "no_reply", "donotreply", "do-not-reply", "mailer-daemon", "postmaster", "bounce"]
        .iter()
        .any(|r| local.contains(r))
}

/// Fold sightings into a ranked list, skipping your own addresses and robots.
pub fn build(seen: impl IntoIterator<Item = Seen>, own: &[String]) -> Vec<Address> {
    struct Acc {
        address: String,
        name: Option<(i64, String)>,
        sent: u32,
        received: u32,
        last: i64,
    }
    let own: Vec<String> = own.iter().map(|a| a.to_lowercase()).collect();
    let mut by: HashMap<String, Acc> = HashMap::new();
    for s in seen {
        let address = s.address.trim();
        let key = address.to_lowercase();
        if !key.contains('@') || own.contains(&key) || is_robot(&key) {
            continue;
        }
        let a = by.entry(key).or_insert_with(|| Acc { address: address.to_string(), name: None, sent: 0, received: 0, last: 0 });
        if s.sent_to {
            a.sent += 1;
        } else {
            a.received += 1;
        }
        a.last = a.last.max(s.date);
        // The most recent name wins: people fix their display names.
        if let Some(n) = s.name.map(|n| n.trim().trim_matches('"').to_string()).filter(|n| !n.is_empty() && !n.contains('@'))
            && a.name.as_ref().is_none_or(|(d, _)| s.date >= *d)
        {
            a.name = Some((s.date, n));
        }
    }
    let mut list: Vec<Acc> = by.into_values().collect();
    list.sort_by(|a, b| {
        let score = |x: &Acc| x.sent * 5 + x.received;
        (b.sent > 0).cmp(&(a.sent > 0)).then(score(b).cmp(&score(a))).then(b.last.cmp(&a.last)).then(a.address.cmp(&b.address))
    });
    list.into_iter().map(|a| Address { name: a.name.map(|(_, n)| n), address: a.address }).collect()
}

/// Up to `limit` contacts matching what's typed, keeping the list's ranking within each tier:
/// address or name-word prefixes first, then anywhere in the address or name.
pub fn matches<'a>(list: &'a [Address], query: &str, skip: &[String], limit: usize) -> Vec<&'a Address> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let skip: Vec<String> = skip.iter().map(|s| s.to_lowercase()).collect();
    let mut prefix = Vec::new();
    let mut inside = Vec::new();
    for c in list {
        let addr = c.address.to_lowercase();
        if skip.contains(&addr) {
            continue;
        }
        let name = c.name.as_deref().unwrap_or("").to_lowercase();
        if addr.starts_with(&q) || name.starts_with(&q) || name.split_whitespace().any(|w| w.starts_with(&q)) {
            prefix.push(c);
        } else if addr.contains(&q) || name.contains(&q) {
            inside.push(c);
        }
        if prefix.len() >= limit {
            break;
        }
    }
    prefix.into_iter().chain(inside).take(limit).collect()
}

/// How a picked contact is written into To/Cc: `Name <addr>`, quoting names with
/// punctuation, and dropping the separators the field splits on.
pub fn recipient(a: &Address) -> String {
    let name: String = a.name.as_deref().unwrap_or("").chars().filter(|c| !matches!(c, ',' | ';' | '"')).collect();
    let name = name.trim();
    if name.is_empty() {
        a.address.clone()
    } else if name.contains(['(', ')', '.', '<', '>', '@', ':', '[', ']', '\\']) {
        format!("\"{name}\" <{}>", a.address)
    } else {
        format!("{name} <{}>", a.address)
    }
}

/// The address in a To/Cc entry: what's inside `<…>`, or the whole thing.
pub fn address_of(entry: &str) -> &str {
    match (entry.rfind('<'), entry.rfind('>')) {
        (Some(a), Some(b)) if a < b => entry[a + 1..b].trim(),
        _ => entry.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(address: &str, name: Option<&str>, date: i64, sent_to: bool) -> Seen {
        Seen { address: address.into(), name: name.map(String::from), date, sent_to }
    }

    #[test]
    fn ranks_people_you_write_to_first() {
        let list = build(
            [
                seen("news@shop.test", Some("Shop"), 10, false),
                seen("news@shop.test", Some("Shop"), 11, false),
                seen("Lucas@x.test", Some("Lucas"), 5, true),
                seen("lucas@x.test", Some("Lucas Freitas"), 6, false),
                seen("me@x.test", Some("Me"), 7, false),
                seen("noreply@bank.test", None, 8, false),
                seen("ana@x.test", None, 1, false),
            ],
            &["ME@x.test".to_string()],
        );
        let addrs: Vec<_> = list.iter().map(|a| a.address.as_str()).collect();
        assert_eq!(addrs, ["Lucas@x.test", "news@shop.test", "ana@x.test"]);
        assert_eq!(list[0].name.as_deref(), Some("Lucas Freitas"), "latest name wins");
    }

    #[test]
    fn recipients() {
        let a = |name: Option<&str>| Address { name: name.map(String::from), address: "x@y.test".into() };
        assert_eq!(recipient(&a(None)), "x@y.test");
        assert_eq!(recipient(&a(Some("Lucas Freitas"))), "Lucas Freitas <x@y.test>");
        assert_eq!(recipient(&a(Some("Freitas, Lucas"))), "Freitas Lucas <x@y.test>");
        assert_eq!(recipient(&a(Some("Lucas F. Silva"))), "\"Lucas F. Silva\" <x@y.test>");
        assert_eq!(address_of(" Lucas <x@y.test> "), "x@y.test");
        assert_eq!(address_of("x@y.test"), "x@y.test");
    }

    #[test]
    fn matching() {
        let list = vec![
            Address { name: Some("Lucas Freitas".into()), address: "lucas.freitas@gmail.test".into() },
            Address { name: Some("Ana Souza".into()), address: "ana@work.test".into() },
            Address { name: None, address: "contact@freitas.test".into() },
        ];
        let found = |q: &str, skip: &[String]| matches(&list, q, skip, 5).iter().map(|a| a.address.clone()).collect::<Vec<_>>();
        assert_eq!(found("fre", &[]), ["lucas.freitas@gmail.test", "contact@freitas.test"], "name word before substring");
        assert_eq!(found("SOU", &[]), ["ana@work.test"]);
        assert_eq!(found("work", &[]), ["ana@work.test"]);
        assert_eq!(found("ana", &["ANA@work.test".into()]), Vec::<String>::new(), "already added");
        assert!(found("  ", &[]).is_empty());
    }
}
