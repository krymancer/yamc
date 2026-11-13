use chrono::{DateTime, Datelike, Local};

fn local(ms: i64) -> Option<DateTime<Local>> {
    DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&Local))
}

/// 14:05 today, "Mar 3" this year, "Mar 3, 2023" before that.
pub fn short_date(ms: i64) -> String {
    let Some(d) = local(ms) else { return String::new() };
    let now = Local::now();
    if d.date_naive() == now.date_naive() {
        d.format("%H:%M").to_string()
    } else if d.year() == now.year() {
        d.format("%b %-d").to_string()
    } else {
        d.format("%b %-d, %Y").to_string()
    }
}

pub fn long_date(ms: i64) -> String {
    local(ms).map(|d| d.format("%b %-d, %Y, %H:%M").to_string()).unwrap_or_default()
}

/// "just now", "5m ago", "3h ago", "2d ago", then the short date.
pub fn ago(ms: i64) -> String {
    let secs = (chrono::Utc::now().timestamp_millis() - ms) / 1000;
    match secs {
        ..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        86_400..604_800 => format!("{}d ago", secs / 86_400),
        _ => short_date(ms),
    }
}
