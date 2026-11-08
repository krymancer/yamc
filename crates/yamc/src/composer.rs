//! Compose / reply. Ctrl+Enter sends, Esc closes (keeping a draft if there's anything in it).
//! Files attach from the Attach button, by dropping them on the composer, or by pasting
//! (an image on the clipboard, or files copied in Finder).

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{self, Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Icon, Sizable, h_flex, v_flex};
use gpui_kit::assets::IconName;
use gpui_kit::*;
use gpui_kit::prelude::*;
use yamc_core::core::Core;
use yamc_core::contacts;
use yamc_core::types::{Account, Address, DraftInput};

use crate::theme::Palette;

pub enum ComposerEvent {
    Close,
    /// Checked and saved as a draft; the app sends it in the background.
    Send(DraftInput),
}

impl EventEmitter<ComposerEvent> for Composer {}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    To,
    Cc,
}

/// Address suggestions for the entry being typed in To or Cc.
struct Suggest {
    field: Field,
    items: Vec<Address>,
    selected: usize,
}

const SUGGESTIONS: usize = 6;

#[derive(Clone, Copy, PartialEq)]
enum Nav {
    Next,
    Prev,
    Accept,
    Dismiss,
}

pub struct Composer {
    core: Arc<Core>,
    draft: DraftInput,
    accounts: Vec<Account>,
    to: Entity<InputState>,
    cc: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    error: Option<String>,
    focus: FocusHandle,
    /// Address book, loaded in the background when the composer opens.
    contacts: Arc<Vec<Address>>,
    suggest: Option<Suggest>,
    _subs: Vec<Subscription>,
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
        let watch = |input: &Entity<InputState>, field: Field, cx: &mut Context<Self>| {
            cx.subscribe(input, move |this: &mut Self, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.refresh_suggest(field, cx);
                }
            })
        };
        let subs = vec![watch(&to, Field::To, cx), watch(&cc, Field::Cc, cx)];
        let book = {
            let core = core.clone();
            core.runtime().clone().spawn_blocking(move || core.contacts())
        };
        cx.spawn(async move |this, cx| {
            if let Ok(list) = book.await {
                let _ = this.update(cx, |this, _| this.contacts = Arc::new(list));
            }
        })
        .detach();
        Self {
            core,
            draft,
            accounts,
            to,
            cc,
            subject,
            body,
            error: None,
            focus: cx.focus_handle(),
            contacts: Arc::default(),
            suggest: None,
            _subs: subs,
        }
    }

    fn input(&self, field: Field) -> &Entity<InputState> {
        match field {
            Field::To => &self.to,
            Field::Cc => &self.cc,
        }
    }

    /// Suggest contacts for the entry after the last comma, leaving out ones already added.
    fn refresh_suggest(&mut self, field: Field, cx: &mut Context<Self>) {
        let text = self.input(field).read(cx).value().to_string();
        let (done, typing) = text.rsplit_once([',', ';']).unwrap_or(("", &text));
        let skip: Vec<String> = done.split([',', ';']).map(|e| contacts::address_of(e).to_string()).collect();
        let items: Vec<Address> = contacts::matches(&self.contacts, typing, &skip, SUGGESTIONS).into_iter().cloned().collect();
        self.suggest = (!items.is_empty()).then_some(Suggest { field, items, selected: 0 });
        cx.notify();
    }

    /// Replace the entry being typed with the picked contact and get ready for the next one.
    fn accept_suggestion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.suggest.take() else { return };
        let Some(pick) = s.items.get(index) else { return };
        let input = self.input(s.field).clone();
        let text = input.read(cx).value().to_string();
        let done = text.rfind([',', ';']).map(|i| text[..=i].trim_end().to_string() + " ").unwrap_or_default();
        let value = format!("{done}{}, ", contacts::recipient(pick));
        input.update(cx, |state, cx| state.set_value(value, window, cx));
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// The suggestions to show: only while their field has focus.
    fn visible_suggest(&self, window: &Window, cx: &App) -> Option<&Suggest> {
        self.suggest.as_ref().filter(|s| self.input(s.field).read(cx).focus_handle(cx).is_focused(window))
    }

    /// ↑/↓ (or ctrl-p/n) move, Enter or Tab accepts, Esc dismisses. Returns whether the
    /// suggestions were showing and took the key.
    fn suggest_nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(len) = self.visible_suggest(window, cx).map(|s| s.items.len()) else { return false };
        match nav {
            Nav::Next | Nav::Prev => {
                if let Some(s) = self.suggest.as_mut() {
                    s.selected = (s.selected + if nav == Nav::Next { 1 } else { len - 1 }) % len;
                }
            }
            Nav::Accept => {
                let i = self.suggest.as_ref().map_or(0, |s| s.selected);
                self.accept_suggestion(i, window, cx);
            }
            Nav::Dismiss => self.suggest = None,
        }
        cx.notify();
        true
    }

    fn take(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        if self.suggest_nav(nav, window, cx) {
            cx.stop_propagation();
        }
    }

    fn render_suggest(&self, field: Field, pal: &Palette, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.visible_suggest(window, cx).filter(|s| s.field == field)?;
        let list = v_flex()
            .absolute()
            .top(px(34.))
            .left(px(84.))
            .w(px(440.))
            .py_1()
            .bg(pal.card)
            .border_1()
            .border_color(pal.line)
            .rounded_md()
            .shadow_lg()
            .occlude()
            .children(s.items.iter().enumerate().map(|(i, a)| {
                let on = i == s.selected;
                h_flex()
                    .id(("suggest", i))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_sm()
                    .cursor_pointer()
                    .when(on, |d| d.bg(pal.active))
                    .when(!on, |d| d.hover(|d| d.bg(pal.hover)))
                    .when_some(a.name.clone(), |d, n| d.child(div().text_color(pal.strong).whitespace_nowrap().child(n)))
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(pal.muted).child(a.address.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.accept_suggestion(i, window, cx);
                        }),
                    )
            }));
        Some(deferred(list).with_priority(1).into_any_element())
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
        d.to.trim().is_empty() && d.subject.trim().is_empty() && d.body.trim().is_empty() && d.attachments.is_empty()
    }

    fn attach(&mut self, paths: impl IntoIterator<Item = std::path::PathBuf>, cx: &mut Context<Self>) {
        for p in paths {
            if p.is_dir() {
                self.error = Some(format!("{} is a folder; zip it to attach", p.display()));
                continue;
            }
            let p = p.to_string_lossy().into_owned();
            if !self.draft.attachments.contains(&p) {
                self.draft.attachments.push(p);
            }
        }
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |this, cx| this.attach(paths, cx));
            }
        })
        .detach();
    }

    /// ⌘V with an image or copied files on the clipboard attaches them. Returns whether it did,
    /// so plain text still pastes into the field.
    fn paste_files(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else { return false };
        let mut handled = false;
        for e in item.entries() {
            match e {
                ClipboardEntry::Image(img) => {
                    let ext = match img.format {
                        ImageFormat::Jpeg => "jpg",
                        ImageFormat::Gif => "gif",
                        ImageFormat::Webp => "webp",
                        ImageFormat::Svg => "svg",
                        ImageFormat::Bmp => "bmp",
                        ImageFormat::Tiff => "tiff",
                        _ => "png",
                    };
                    match self.core.save_pasted(&img.bytes, ext) {
                        Ok(p) => self.draft.attachments.push(p),
                        Err(e) => self.error = Some(format!("can't paste image: {e:#}")),
                    }
                    handled = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    self.attach(paths.paths().to_vec(), cx);
                    handled = true;
                }
                ClipboardEntry::String(_) => {}
            }
        }
        if handled {
            cx.notify();
        }
        handled
    }

    fn remove_attachment(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.draft.attachments.len() {
            let p = self.draft.attachments.remove(i);
            // A pasted image only exists for this message.
            if p.contains("/pasted/Image ") {
                let _ = std::fs::remove_file(&p);
            }
        }
        cx.notify();
    }

    fn render_attachments(&self, pal: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.draft.attachments.is_empty() {
            return None;
        }
        Some(h_flex().px_4().py_2().gap_2().flex_wrap().border_t_1().border_color(pal.line).children(
            self.draft.attachments.iter().enumerate().map(|(i, p)| {
                let path = std::path::Path::new(p);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
                let size = std::fs::metadata(path).map(|m| format_size(m.len())).unwrap_or_else(|_| "missing".into());
                h_flex()
                    .gap_1p5()
                    .pl_2()
                    .pr_1()
                    .py_0p5()
                    .max_w(px(260.))
                    .rounded_md()
                    .border_1()
                    .border_color(pal.line)
                    .text_xs()
                    .text_color(pal.text)
                    .child(Icon::new(IconName::Paperclip).size_3().text_color(pal.faint))
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(name))
                    .child(div().flex_none().text_color(pal.faint).child(size))
                    .child(
                        div()
                            .id(("unattach", i))
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(pal.faint)
                            .hover(|s| s.bg(pal.hover).text_color(pal.strong))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_attachment(i, cx)))
                            .child(Icon::new(IconName::X).size_3()),
                    )
            }),
        ))
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
        while !self.draft.attachments.is_empty() {
            self.remove_attachment(0, cx);
        }
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

    /// Close right away and send in the background. The message is saved as a draft first,
    /// so a failed send (or a crash mid-send) leaves it in Drafts.
    pub fn send(&mut self, cx: &mut Context<Self>) {
        let mut d = self.current(cx);
        let saved = self.core.check_sendable(&d).and_then(|_| self.core.save_draft(&d));
        match saved {
            Ok(id) => {
                d.id = Some(id);
                self.draft.id = Some(id);
                cx.emit(ComposerEvent::Send(d));
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                cx.notify();
            }
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = cx.global::<Palette>().clone();
        let from = self
            .accounts
            .iter()
            .find(|a| a.id == self.draft.account_id)
            .map(|a| if a.sender.is_empty() { a.email.clone() } else { format!("{} <{}>", a.sender, a.email) })
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
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                let emacs = k.modifiers.control && !k.modifiers.platform && !k.modifiers.alt;
                let nav = match k.key.as_str() {
                    "n" if emacs => Some(Nav::Next),
                    "p" if emacs => Some(Nav::Prev),
                    _ => None,
                };
                if nav.is_some_and(|n| this.suggest_nav(n, window, cx)) {
                    cx.stop_propagation();
                } else if k.key == "enter" && (k.modifiers.control || k.modifiers.platform) {
                    cx.stop_propagation();
                    this.send(cx);
                } else if k.key == "escape" {
                    cx.stop_propagation();
                    this.close(cx);
                }
            }))
            // The text field binds ↑/↓, Enter, Tab and Esc to its own actions, which run before key
            // listeners; take them while suggestions are showing.
            .capture_action(cx.listener(|this, _: &input::MoveDown, window, cx| this.take(Nav::Next, window, cx)))
            .capture_action(cx.listener(|this, _: &input::MoveUp, window, cx| this.take(Nav::Prev, window, cx)))
            .capture_action(cx.listener(|this, _: &input::IndentInline, window, cx| this.take(Nav::Accept, window, cx)))
            .capture_action(cx.listener(|this, e: &input::Enter, window, cx| {
                if !e.secondary && !e.shift {
                    this.take(Nav::Accept, window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &input::Escape, window, cx| this.take(Nav::Dismiss, window, cx)))
            // Paste arrives as an action (from ⌘V or Edit > Paste); take it before the text field
            // does when the clipboard holds an image or files, otherwise let text paste as usual.
            .capture_action(cx.listener(|this, _: &gpui_kit::component::input::Paste, _, cx| {
                if this.paste_files(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.attach(paths.paths().to_vec(), cx)))
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
            .child(row("To", Input::new(&self.to).appearance(false).small().into_any_element()).relative().children(self.render_suggest(Field::To, &pal, window, cx)))
            .child(row("Cc", Input::new(&self.cc).appearance(false).small().into_any_element()).relative().children(self.render_suggest(Field::Cc, &pal, window, cx)))
            .child(row("Subject", Input::new(&self.subject).appearance(false).small().into_any_element()))
            .child(div().flex_1().min_h_0().px_2().py_2().child(Textarea::new(&self.body).appearance(false).h_full()))
            .children(self.render_attachments(&pal, cx))
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
                            .label("Send")
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    )
                    .child(
                        Button::new("attach")
                            .ghost()
                            .small()
                            .icon(IconName::Paperclip)
                            .label("Attach")
                            .tooltip("Attach files (or paste / drop them here)")
                            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
                    )
                    .child(Button::new("save").ghost().small().label("Save draft").on_click(cx.listener(|this, _, _, cx| this.save(cx))))
                    .child(div().flex_1())
                    .child(Button::new("discard").ghost().small().label("Discard").on_click(cx.listener(|this, _, _, cx| this.discard(cx)))),
            )
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    }
}
