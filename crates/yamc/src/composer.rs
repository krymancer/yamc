//! Compose / reply. Ctrl+Enter sends, Esc closes (keeping a draft if there's anything in it).

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::*;
use gpui_kit::prelude::*;
use gpui_kit::component::Disableable as _;
use yamc_core::core::Core;
use yamc_core::types::{Account, DraftInput};

use crate::theme::Palette;

pub enum ComposerEvent {
    Close,
    Sent,
}

impl EventEmitter<ComposerEvent> for Composer {}

pub struct Composer {
    core: Arc<Core>,
    draft: DraftInput,
    accounts: Vec<Account>,
    to: Entity<InputState>,
    cc: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    busy: bool,
    error: Option<String>,
    focus: FocusHandle,
}

impl Composer {
    pub fn new(core: Arc<Core>, draft: DraftInput, accounts: Vec<Account>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |v: &str, ph: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            let v = v.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(ph).default_value(v))
        };
        let to = field(&draft.to, "Recipients", window, cx);
        let cc = field(&draft.cc, "Cc", window, cx);
        let subject = field(&draft.subject, "Subject", window, cx);
        let body_text = draft.body.clone();
        let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("Write…").default_value(body_text));
        // Replies start in the body, new mail in To.
        let first = if draft.to.is_empty() { to.read(cx).focus_handle(cx) } else { body.read(cx).focus_handle(cx) };
        window.focus(&first, cx);
        if draft.reply_to.is_some() {
            body.update(cx, |b, cx| b.set_cursor_position(gpui_kit::component::input::Position::new(0, 0), window, cx));
        }
        Self { core, draft, accounts, to, cc, subject, body, busy: false, error: None, focus: cx.focus_handle() }
    }

    fn current(&self, cx: &App) -> DraftInput {
        DraftInput {
            to: self.to.read(cx).value().to_string(),
            cc: self.cc.read(cx).value().to_string(),
            subject: self.subject.read(cx).value().to_string(),
            body: self.body.read(cx).value().to_string(),
            ..self.draft.clone()
        }
    }

    fn is_blank(d: &DraftInput) -> bool {
        d.to.trim().is_empty() && d.subject.trim().is_empty() && d.body.trim().is_empty()
    }

    /// Keep what's typed as a draft and close.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        let d = self.current(cx);
        let unchanged = self.draft.id.is_none() && d.body == self.draft.body && d.to == self.draft.to && self.draft.reply_to.is_some();
        if !Self::is_blank(&d) && !unchanged {
            let _ = self.core.save_draft(&d);
        }
        cx.emit(ComposerEvent::Close);
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.draft.id {
            let _ = self.core.delete_draft(id);
        }
        cx.emit(ComposerEvent::Close);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let d = self.current(cx);
        match self.core.save_draft(&d) {
            Ok(id) => {
                self.draft.id = Some(id);
                cx.emit(ComposerEvent::Close);
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
        cx.notify();
    }

    pub fn send(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let d = self.current(cx);
        self.busy = true;
        self.error = None;
        cx.notify();
        let core = self.core.clone();
        let task = self.core.runtime().spawn(async move { core.send(&d).await });
        cx.spawn(async move |this, cx| {
            let r = task.await.map_err(anyhow::Error::from).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match r {
                    Ok(()) => cx.emit(ComposerEvent::Sent),
                    Err(e) => this.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cycle_account(&mut self, cx: &mut Context<Self>) {
        if self.accounts.len() < 2 {
            return;
        }
        let i = self.accounts.iter().position(|a| a.id == self.draft.account_id).unwrap_or(0);
        self.draft.account_id = self.accounts[(i + 1) % self.accounts.len()].id;
        cx.notify();
    }
}

impl Focusable for Composer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Composer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = cx.global::<Palette>().clone();
        let from = self
            .accounts
            .iter()
            .find(|a| a.id == self.draft.account_id)
            .map(|a| format!("{} <{}>", a.name, a.email))
            .unwrap_or_default();
        let row = |label: &'static str, input: AnyElement| {
            h_flex()
                .gap_3()
                .px_4()
                .py_1()
                .border_b_1()
                .border_color(pal.line)
                .child(div().w(px(56.)).text_xs().text_color(pal.faint).child(label))
                .child(div().flex_1().child(input))
        };
        v_flex()
            .track_focus(&self.focus)
            .key_context("Composer")
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.key == "enter" && (k.modifiers.control || k.modifiers.platform) {
                    cx.stop_propagation();
                    this.send(cx);
                } else if k.key == "escape" {
                    cx.stop_propagation();
                    this.close(cx);
                }
            }))
            .w(px(640.))
            .h(px(520.))
            .bg(pal.card)
            .border_1()
            .border_color(pal.line)
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(pal.line)
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(pal.strong)
                            .child(if self.draft.reply_to.is_some() { "Reply" } else { "New message" }),
                    )
                    .child(div().text_xs().text_color(pal.faint).child("⌃↵ send · esc close")),
            )
            .child(row(
                "From",
                div()
                    .id("from")
                    .text_sm()
                    .text_color(pal.text)
                    .py_1()
                    .when(self.accounts.len() > 1, |d| d.cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_account(cx)))
                    .child(from)
                    .into_any_element(),
            ))
            .child(row("To", Input::new(&self.to).appearance(false).small().into_any_element()))
            .child(row("Cc", Input::new(&self.cc).appearance(false).small().into_any_element()))
            .child(row("Subject", Input::new(&self.subject).appearance(false).small().into_any_element()))
            .child(div().flex_1().min_h_0().px_2().py_2().child(Textarea::new(&self.body).appearance(false).h_full()))
            .when_some(self.error.clone(), |d, e| d.child(div().px_4().py_2().text_sm().text_color(pal.danger).child(e)))
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(pal.line)
                    .child(
                        Button::new("send")
                            .primary()
                            .small()
                            .label(if self.busy { "Sending…" } else { "Send" })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    )
                    .child(Button::new("save").ghost().small().label("Save draft").on_click(cx.listener(|this, _, _, cx| this.save(cx))))
                    .child(div().flex_1())
                    .child(Button::new("discard").ghost().small().label("Discard").on_click(cx.listener(|this, _, _, cx| this.discard(cx)))),
            )
    }
}
