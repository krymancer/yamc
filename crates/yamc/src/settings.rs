//! Settings: accounts, appearance, sync, and MCP for coding agents.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable, h_flex, v_flex};
use gpui_kit::assets::IconName;
use gpui_kit::*;
use gpui_kit::prelude::*;
use gpui_kit::component::Disableable as _;
use yamc_core::core::Core;
use yamc_core::presets::{PRESETS, preset_config};
use yamc_core::types::{Account, AccountConfig, AccountInput, AccountKind, Settings, TlsMode};

use crate::theme::{CAT_ACCENTS, Palette, THEME_IDS, cat_swatch, label, palette};

struct AddForm {
    kind: AccountKind,
    email: Entity<InputState>,
    name: Entity<InputState>,
    sender: Entity<InputState>,
    password: Entity<InputState>,
    username: Entity<InputState>,
    imap_host: Entity<InputState>,
    imap_port: Entity<InputState>,
    imap_tls: TlsMode,
    smtp_host: Entity<InputState>,
    smtp_port: Entity<InputState>,
    smtp_tls: TlsMode,
    advanced: bool,
    busy: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

struct Renaming {
    id: i64,
    label: Entity<InputState>,
    sender: Entity<InputState>,
    _subs: [Subscription; 2],
}

pub struct SettingsView {
    core: Arc<Core>,
    form: Option<AddForm>,
    /// Account whose label and sender name are being edited.
    renaming: Option<Renaming>,
    body_days: Entity<InputState>,
    copied: Option<&'static str>,
    pub scroll: ScrollHandle,
}

fn tls_label(t: TlsMode) -> &'static str {
    match t {
        TlsMode::Tls => "TLS",
        TlsMode::Starttls => "STARTTLS",
        TlsMode::None => "None",
    }
}

impl SettingsView {
    pub fn new(core: Arc<Core>, adding: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let days = core.get_settings().body_days.to_string();
        let body_days = cx.new(|cx| InputState::new(window, cx).default_value(days));
        cx.subscribe(&body_days, |this: &mut Self, input, e: &gpui_kit::component::input::InputEvent, cx| {
            if matches!(e, gpui_kit::component::input::InputEvent::Change)
                && let Ok(n) = input.read(cx).value().trim().parse::<i64>()
                && (1..=3650).contains(&n)
            {
                let _ = this.core.set_settings(serde_json::json!({ "bodyDays": n }));
            }
        })
        .detach();
        let mut this = Self { core, form: None, renaming: None, body_days, copied: None, scroll: ScrollHandle::new() };
        if adding {
            this.open_form(window, cx);
        }
        this
    }

    fn start_rename(&mut self, a: &Account, window: &mut Window, cx: &mut Context<Self>) {
        let label = cx.new(|cx| InputState::new(window, cx).placeholder(a.email.clone()).default_value(a.name.clone()));
        // Unset shows what yamc infers, so clearing the field visibly means "use that".
        let hint = if a.sender.is_empty() { "Your name".to_string() } else { a.sender.clone() };
        let sender = cx.new(|cx| {
            InputState::new(window, cx).placeholder(hint).default_value(a.sender_name.clone().unwrap_or_default())
        });
        let enter = |input: &Entity<InputState>, cx: &mut Context<Self>| {
            cx.subscribe(input, |this: &mut Self, _, e: &gpui_kit::component::input::InputEvent, cx| {
                if matches!(e, gpui_kit::component::input::InputEvent::PressEnter { .. }) {
                    this.finish_rename(true, cx);
                }
            })
        };
        let subs = [enter(&label, cx), enter(&sender, cx)];
        window.focus(&label.read(cx).focus_handle(cx), cx);
        label.update(cx, |s, cx| s.select_all(window, cx));
        self.renaming = Some(Renaming { id: a.id, label, sender, _subs: subs });
        cx.notify();
    }

    fn finish_rename(&mut self, save: bool, cx: &mut Context<Self>) {
        if let Some(r) = self.renaming.take()
            && save
        {
            let _ = self.core.rename_account(r.id, &r.label.read(cx).value(), &r.sender.read(cx).value());
        }
        cx.notify();
    }

    pub fn open_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = |ph: &'static str, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).placeholder(ph));
        let email = input("you@example.com", window, cx);
        let cfg = preset_config(AccountKind::Gmail, "");
        let sub = cx.subscribe(&email, |this: &mut Self, _, e: &gpui_kit::component::input::InputEvent, cx| {
            if matches!(e, gpui_kit::component::input::InputEvent::Change) {
                this.refill(cx);
            }
        });
        let form = AddForm {
            kind: AccountKind::Gmail,
            name: input("Gmail, Work…", window, cx),
            sender: input("Inferred from your sent mail", window, cx),
            password: cx.new(|cx| InputState::new(window, cx).masked(true)),
            username: input("username", window, cx),
            imap_host: cx.new(|cx| InputState::new(window, cx).default_value(cfg.imap.host.clone())),
            imap_port: cx.new(|cx| InputState::new(window, cx).default_value(cfg.imap.port.to_string())),
            imap_tls: cfg.imap.tls,
            smtp_host: cx.new(|cx| InputState::new(window, cx).default_value(cfg.smtp.host.clone())),
            smtp_port: cx.new(|cx| InputState::new(window, cx).default_value(cfg.smtp.port.to_string())),
            smtp_tls: cfg.smtp.tls,
            email,
            advanced: false,
            busy: false,
            error: None,
            _subs: vec![sub],
        };
        window.focus(&form.email.read(cx).focus_handle(cx), cx);
        self.form = Some(form);
        cx.notify();
    }

    /// Re-derive server settings from the preset and email.
    fn refill(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.form.as_mut() else { return };
        let email = f.email.read(cx).value().to_string();
        let cfg = preset_config(f.kind, &email);
        f.imap_tls = cfg.imap.tls;
        f.smtp_tls = cfg.smtp.tls;
        let pending = vec![
            (f.username.clone(), cfg.username.clone()),
            (f.imap_host.clone(), cfg.imap.host.clone()),
            (f.imap_port.clone(), cfg.imap.port.to_string()),
            (f.smtp_host.clone(), cfg.smtp.host.clone()),
            (f.smtp_port.clone(), cfg.smtp.port.to_string()),
        ];
        cx.defer_in_window_or_skip(pending);
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.form.as_mut() else { return };
        let email = f.email.read(cx).value().trim().to_string();
        let password = f.password.read(cx).value().to_string();
        if email.is_empty() || password.is_empty() {
            f.error = Some("Email and password are required.".into());
            cx.notify();
            return;
        }
        let preset = preset_config(f.kind, &email);
        let username = f.username.read(cx).value().trim().to_string();
        let port = |e: &Entity<InputState>, cx: &App, d: u16| e.read(cx).value().trim().parse().unwrap_or(d);
        let config = AccountConfig {
            imap: yamc_core::types::ServerConfig {
                host: f.imap_host.read(cx).value().trim().to_string(),
                port: port(&f.imap_port, cx, preset.imap.port),
                tls: f.imap_tls,
            },
            smtp: yamc_core::types::ServerConfig {
                host: f.smtp_host.read(cx).value().trim().to_string(),
                port: port(&f.smtp_port, cx, preset.smtp.port),
                tls: f.smtp_tls,
            },
            username: if username.is_empty() { email.clone() } else { username },
            ..preset
        };
        let input = AccountInput {
            name: f.name.read(cx).value().trim().to_string(),
            sender_name: f.sender.read(cx).value().trim().to_string(),
            email,
            kind: f.kind,
            config,
            password,
        };
        f.busy = true;
        f.error = None;
        cx.notify();
        let core = self.core.clone();
        let task = self.core.runtime().spawn(async move { core.add_account(input).await });
        cx.spawn(async move |this, cx| {
            let r = task.await.map_err(anyhow::Error::from).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(_) => this.form = None,
                    Err(e) => {
                        if let Some(f) = this.form.as_mut() {
                            f.busy = false;
                            f.error = Some(format!("{e:#}"));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn copy(&mut self, what: &'static str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(what);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1200)).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn set(&self, patch: serde_json::Value) {
        let _ = self.core.set_settings(patch);
    }

    fn section(title: &'static str, desc: &'static str, pal: &Palette) -> Div {
        v_flex()
            .gap_1()
            .pb_3()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).text_color(pal.strong).child(title))
            .child(div().text_sm().text_color(pal.muted).child(desc))
    }

    fn render_accounts(&mut self, accounts: &[Account], pal: &Palette, cx: &mut Context<Self>) -> Div {
        let list = v_flex().border_1().border_color(pal.line).rounded_lg().bg(pal.card).children(accounts.iter().enumerate().map(|(i, a)| {
            let id = a.id;
            let state = match (&a.state, &a.error) {
                (_, Some(e)) => e.clone(),
                (s, None) => format!("{s:?}").to_lowercase(),
            };
            let editing = self.renaming.as_ref().filter(|r| r.id == id).map(|r| (r.label.clone(), r.sender.clone()));
            let account = a.clone();
            let title: AnyElement = match &editing {
                Some((label, sender)) => h_flex()
                    .gap_2()
                    .pb_1()
                    .child(v_flex().gap_1().w(px(200.)).child(div().text_xs().text_color(pal.faint).child("Label")).child(Input::new(label).small()))
                    .child(v_flex().gap_1().w(px(200.)).child(div().text_xs().text_color(pal.faint).child("Sender name")).child(Input::new(sender).small()))
                    .child(div().pt_4().flex().gap_2().items_center()
                    .child(
                        Button::new(("rename-save", id as u64))
                            .small()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.finish_rename(true, cx))),
                    )
                    .child(
                        Button::new(("rename-cancel", id as u64))
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.finish_rename(false, cx))),
                    ))
                    .into_any_element(),
                None => {
                    let label = if a.name.is_empty() { a.email.clone() } else { format!("{} · {}", a.name, a.email) };
                    let sends_as = match (&a.sender_name, a.sender.is_empty()) {
                        (_, true) => "Sends with no name — set one with ✎".to_string(),
                        (Some(_), false) => format!("Sends as {}", a.sender),
                        (None, false) => format!("Sends as {} (from your sent mail)", a.sender),
                    };
                    v_flex()
                        .child(div().text_sm().text_color(pal.strong).child(label))
                        .child(div().text_xs().text_color(pal.muted).child(sends_as))
                        .into_any_element()
                }
            };
            h_flex()
                .px_4()
                .py_3()
                .gap_3()
                .when(i > 0, |d| d.border_t_1().border_color(pal.line))
                .child(div().size(px(8.)).rounded_full().bg(pal.account(i)))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(title)
                        .child(div().text_xs().text_color(pal.faint).overflow_hidden().text_ellipsis().whitespace_nowrap().child(format!(
                            "{} · {}:{} · {state}",
                            a.kind.as_str(),
                            a.config.imap.host,
                            a.config.imap.port
                        ))),
                )
                .when(editing.is_none(), |d| {
                    d.child(
                        Button::new(("rename", id as u64))
                            .ghost()
                            .small()
                            .icon(IconName::Pencil)
                            .tooltip("Edit label and sender name")
                            .on_click(cx.listener(move |this, _, window, cx| this.start_rename(&account, window, cx))),
                    )
                })
                .child(
                    Button::new(("remove", id as u64))
                        .ghost()
                        .small()
                        .icon(IconName::Trash)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let _ = this.core.remove_account(id);
                            cx.notify();
                        })),
                )
        }));
        let mut col = v_flex().gap_3().child(Self::section("Accounts", "Each account keeps its own folders. The inbox shows them all together.", pal));
        if !accounts.is_empty() {
            col = col.child(list);
        }
        match &self.form {
            Some(_) => col.child(self.render_form(pal, cx)),
            None => col.child(
                h_flex().child(
                    Button::new("add")
                        .outline()
                        .small()
                        .icon(IconName::Plus)
                        .label("Add account")
                        .on_click(cx.listener(|this, _, window, cx| this.open_form(window, cx))),
                ),
            ),
        }
    }

    fn field(label: &'static str, input: impl IntoElement, pal: &Palette) -> Div {
        v_flex().gap_1().child(div().text_xs().text_color(pal.muted).child(label)).child(input)
    }

    fn tls_picker(&self, which: &'static str, current: TlsMode, cx: &mut Context<Self>) -> Div {
        h_flex().gap_1().children([TlsMode::Tls, TlsMode::Starttls, TlsMode::None].into_iter().map(|t| {
            let b = Button::new(SharedString::from(format!("{which}-{t:?}"))).small().label(tls_label(t));
            let b = if t == current { b.primary() } else { b.ghost() };
            b.on_click(cx.listener(move |this, _, _, cx| {
                if let Some(f) = this.form.as_mut() {
                    if which == "imap" { f.imap_tls = t } else { f.smtp_tls = t }
                }
                cx.notify();
            }))
        }))
    }

    fn render_form(&mut self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let f = self.form.as_ref().unwrap();
        let preset = PRESETS.iter().find(|p| p.kind == f.kind).unwrap();
        let kinds = h_flex().gap_1().p_1().rounded_md().bg(pal.hover).children(PRESETS.iter().map(|p| {
            let kind = p.kind;
            let b = Button::new(SharedString::from(format!("kind-{}", p.kind.as_str()))).small().label(p.label).disabled(p.disabled);
            let b = if kind == f.kind { b.primary() } else { b.ghost() };
            b.on_click(cx.listener(move |this, _, _, cx| {
                if let Some(f) = this.form.as_mut() {
                    f.kind = kind;
                    f.advanced = kind == AccountKind::Imap;
                }
                this.refill(cx);
            }))
        }));
        let advanced = f.advanced || f.kind == AccountKind::Imap;
        let server = |label: &'static str, host: &Entity<InputState>, port: &Entity<InputState>, picker: Div| {
            Self::field(
                label,
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(host).small()))
                    .child(div().w(px(80.)).child(Input::new(port).small()))
                    .child(picker),
                pal,
            )
        };
        let imap_picker = self.tls_picker("imap", f.imap_tls, cx);
        let smtp_picker = self.tls_picker("smtp", f.smtp_tls, cx);
        let f = self.form.as_ref().unwrap();
        v_flex()
            .gap_3()
            .p_4()
            .border_1()
            .border_color(pal.line)
            .rounded_lg()
            .bg(pal.card)
            .child(kinds)
            .child(div().text_sm().text_color(pal.muted).child(preset.help))
            .child(Self::field("Email", Input::new(&f.email).small(), pal))
            .child(Self::field("Label", Input::new(&f.name).small(), pal))
            .child(Self::field("Sender name", Input::new(&f.sender).small(), pal))
            .child(Self::field(preset.password_label, Input::new(&f.password).small(), pal))
            .when(advanced, |d| {
                d.child(Self::field("Username", Input::new(&f.username).small(), pal))
                    .child(server("Incoming (IMAP)", &f.imap_host, &f.imap_port, imap_picker))
                    .child(server("Outgoing (SMTP)", &f.smtp_host, &f.smtp_port, smtp_picker))
            })
            .when(!advanced, |d| {
                d.child(h_flex().child(Button::new("advanced").ghost().small().label("Server settings…").on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(f) = this.form.as_mut() {
                            f.advanced = true;
                        }
                        cx.notify();
                    },
                ))))
            })
            .when_some(f.error.clone(), |d, e| {
                d.child(div().text_sm().text_color(pal.danger).p_2().rounded_md().border_1().border_color(pal.danger).child(e))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("connect")
                            .primary()
                            .small()
                            .label(if f.busy { "Connecting…" } else { "Add account" })
                            .disabled(f.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    )
                    .child(Button::new("cancel").ghost().small().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                        this.form = None;
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }

    fn render_appearance(&self, s: &Settings, pal: &Palette, window: &Window, cx: &mut Context<Self>) -> Div {
        let sys_dark = crate::theme::system_dark(Some(window), cx);
        let cards = h_flex().flex_wrap().gap_3().children(THEME_IDS.iter().copied().chain(["system"]).map(|id| {
            let p = palette(id, &s.accent, sys_dark);
            let on = s.theme == id;
            v_flex()
                .id(SharedString::from(format!("theme-{id}")))
                .w(px(132.))
                .gap_2()
                .p_2()
                .rounded_lg()
                .border_1()
                .border_color(if on { pal.ring } else { pal.line })
                .when(on, |d| d.bg(pal.hover))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, _| this.set(serde_json::json!({ "theme": id }))))
                .child(
                    v_flex()
                        .h(px(64.))
                        .rounded_md()
                        .bg(p.bg)
                        .border_1()
                        .border_color(p.line)
                        .p_2()
                        .gap_1()
                        .child(div().h(px(6.)).w(px(56.)).rounded_sm().bg(p.strong))
                        .child(div().h(px(6.)).w(px(84.)).rounded_sm().bg(p.muted))
                        .child(div().h(px(6.)).w(px(70.)).rounded_sm().bg(p.faint))
                        .child(div().h(px(6.)).w(px(24.)).rounded_sm().bg(p.accent)),
                )
                .child(div().text_xs().text_color(if on { pal.strong } else { pal.muted }).child(label(id)))
        }));
        let is_cat = ["latte", "frappe", "macchiato", "mocha"].contains(&s.theme.as_str());
        let swatches = h_flex().flex_wrap().gap_2().children(CAT_ACCENTS.iter().map(|a| {
            let color = cat_swatch(if is_cat { &s.theme } else { "mocha" }, a).unwrap_or(pal.accent);
            let on = s.accent == *a;
            div()
                .id(SharedString::from(format!("accent-{a}")))
                .size(px(22.))
                .rounded_full()
                .bg(color)
                .border_2()
                .border_color(if on { pal.strong } else { pal.bg })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, _| this.set(serde_json::json!({ "accent": a }))))
        }));
        v_flex()
            .gap_3()
            .child(Self::section("Appearance", "Mail is drawn natively in these colours, so nothing flashes white.", pal))
            .child(cards)
            .when(is_cat, |d| d.child(div().text_xs().text_color(pal.muted).pt_1().child("Accent")).child(swatches))
    }

    fn render_sync(&self, pal: &Palette) -> Div {
        v_flex()
            .gap_3()
            .child(Self::section("Sync", "Headers sync for everything. Bodies download ahead of time for recent mail; older ones when you open them.", pal))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(90.)).child(Input::new(&self.body_days).small()))
                    .child(div().text_sm().text_color(pal.muted).child("days of bodies kept offline")),
            )
    }

    fn render_mcp(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let info = self.core.mcp_info();
        let enabled = info.enabled;
        let status = if info.running {
            format!("Running on {}", info.url)
        } else if let Some(e) = &info.error {
            format!("Not running: {e}")
        } else {
            "Off".into()
        };
        let code = |id: &'static str, text: String, cx: &mut Context<Self>| {
            let copied = self.copied == Some(id);
            let t = text.clone();
            h_flex()
                .gap_2()
                .items_start()
                .p_3()
                .rounded_md()
                .bg(pal.panel)
                .border_1()
                .border_color(pal.line)
                .child(div().flex_1().min_w_0().font_family("monospace").text_xs().text_color(pal.text).child(text))
                .child(
                    Button::new(id)
                        .ghost()
                        .xsmall()
                        .icon(if copied { IconName::Check } else { IconName::Copy })
                        .label(if copied { "Copied" } else { "Copy" })
                        .on_click(cx.listener(move |this, _, _, cx| this.copy(id, t.clone(), cx))),
                )
        };
        v_flex()
            .gap_3()
            .child(Self::section(
                "AI agents (MCP)",
                "Claude Code, Codex and other agents can read, search, archive and write drafts. They can't send; you review drafts here.",
                pal,
            ))
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("mcp-toggle")
                            .small()
                            .when(enabled, |b| b.primary())
                            .when(!enabled, |b| b.outline())
                            .label(if enabled { "Enabled" } else { "Disabled" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set(serde_json::json!({ "mcpEnabled": !enabled }));
                                cx.notify();
                            })),
                    )
                    .child(div().text_sm().text_color(if info.error.is_some() { pal.danger } else { pal.muted }).child(status)),
            )
            .when(enabled, |d| {
                d.child(div().text_xs().text_color(pal.muted).child("Claude Code"))
                    .child(code("copy-claude", info.claude_command.clone(), cx))
                    .child(div().text_xs().text_color(pal.muted).child("Codex (~/.codex/config.toml)"))
                    .child(code("copy-codex", info.codex_config.clone(), cx))
                    .child(
                        h_flex().child(Button::new("rotate").ghost().small().label("New token").on_click(cx.listener(|this, _, _, cx| {
                            this.set(serde_json::json!({ "mcpToken": yamc_core::secrets::random_token() }));
                            cx.notify();
                        }))),
                    )
            })
    }
}

trait DeferFill {
    fn defer_in_window_or_skip(&mut self, pending: Vec<(Entity<InputState>, String)>);
}

impl DeferFill for Context<'_, SettingsView> {
    /// Input values can only be set with a window; do it on the next frame.
    fn defer_in_window_or_skip(&mut self, pending: Vec<(Entity<InputState>, String)>) {
        let this = self.entity().downgrade();
        self.defer(move |cx| {
            let Some(window) = cx.active_window() else { return };
            let _ = window.update(cx, |_, window, cx| {
                for (e, v) in pending {
                    e.update(cx, |s, cx| s.set_value(v, window, cx));
                }
            });
            let _ = this;
        });
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = cx.global::<Palette>().clone();
        let s = self.core.get_settings();
        let accounts = self.core.list_accounts();
        let sep = || div().h(px(1.)).bg(pal.line).my_6();
        let accounts_el = self.render_accounts(&accounts, &pal, cx);
        let appearance = self.render_appearance(&s, &pal, window, cx);
        let mcp = self.render_mcp(&pal, cx);
        div().id("settings-scroll").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(
            v_flex()
                .w_full()
                .max_w(px(720.))
                .mx_auto()
                .px_6()
                .py_6()
                .child(accounts_el)
                .child(sep())
                .child(appearance)
                .child(sep())
                .child(self.render_sync(&pal))
                .child(sep())
                .child(mcp)
                .child(sep())
                .child(
                    h_flex()
                        .gap_2()
                        .pb_6()
                        .text_xs()
                        .text_color(pal.faint)
                        .child(Icon::new(IconName::Info).size_3())
                        .child(format!("Data in {} · passwords sealed with the {}", self.core.data_dir.display(), self.core.secret_source)),
                ),
        )
    }
}
