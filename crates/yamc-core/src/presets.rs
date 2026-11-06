use crate::types::{AccountConfig, AccountKind, ServerConfig, TlsMode};

pub struct Preset {
    pub kind: AccountKind,
    pub label: &'static str,
    pub help: &'static str,
    pub password_label: &'static str,
    /// Not usable yet (needs OAuth).
    pub disabled: bool,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        kind: AccountKind::Gmail,
        label: "Gmail",
        help: "Use an app password (Google Account → Security → 2-Step Verification → App passwords).",
        password_label: "App password",
        disabled: false,
    },
    Preset {
        kind: AccountKind::Proton,
        label: "Proton (Bridge)",
        help: "Proton Mail Bridge must be running. Use the username and password Bridge shows, not your Proton password.",
        password_label: "Bridge password",
        disabled: false,
    },
    Preset {
        kind: AccountKind::Outlook,
        label: "Outlook",
        help: "Outlook requires OAuth sign-in, which lands in milestone 2.",
        password_label: "Password",
        disabled: true,
    },
    Preset {
        kind: AccountKind::Imap,
        label: "Other (IMAP)",
        help: "Any IMAP/SMTP server.",
        password_label: "Password",
        disabled: false,
    },
];

fn server(host: &str, port: u16, tls: TlsMode) -> ServerConfig {
    ServerConfig { host: host.into(), port, tls }
}

pub fn preset_config(kind: AccountKind, email: &str) -> AccountConfig {
    let username = email.to_string();
    match kind {
        AccountKind::Gmail => AccountConfig {
            imap: server("imap.gmail.com", 993, TlsMode::Tls),
            smtp: server("smtp.gmail.com", 465, TlsMode::Tls),
            username,
            allow_self_signed: false,
            server_saves_sent: true,
        },
        AccountKind::Proton => AccountConfig {
            imap: server("127.0.0.1", 1143, TlsMode::Starttls),
            smtp: server("127.0.0.1", 1025, TlsMode::Starttls),
            username,
            allow_self_signed: true,
            server_saves_sent: true,
        },
        AccountKind::Outlook => AccountConfig {
            imap: server("outlook.office365.com", 993, TlsMode::Tls),
            smtp: server("smtp.office365.com", 587, TlsMode::Starttls),
            username,
            allow_self_signed: false,
            server_saves_sent: true,
        },
        AccountKind::Imap => {
            let domain = email.split_once('@').map(|(_, d)| d).unwrap_or("");
            let host = |p: &str| if domain.is_empty() { String::new() } else { format!("{p}.{domain}") };
            AccountConfig {
                imap: ServerConfig { host: host("imap"), port: 993, tls: TlsMode::Tls },
                smtp: ServerConfig { host: host("smtp"), port: 465, tls: TlsMode::Tls },
                username,
                allow_self_signed: false,
                server_saves_sent: false,
            }
        }
    }
}
