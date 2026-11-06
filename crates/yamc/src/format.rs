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
