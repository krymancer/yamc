//! The window: sidebar, top bar, list / conversation / drafts / settings, vim keys.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable, h_flex, v_flex};
use gpui_kit::assets::IconName;
use gpui_kit::*;
use gpui_kit::prelude::*;
use yamc_core::core::Core;
use yamc_core::types::{Account, CoreEvent, Draft, DraftInput, Mailbox, ThreadRow, View};

use crate::body::Images;
use crate::composer::{Composer, ComposerEvent};
use crate::format::short_date;
use crate::keys::{BINDINGS, Cmd, Feed, KeyEngine, Mode, key_token, pretty};
use crate::menus;
use crate::list::{PAGE, ROW, Screen as ScreenPos, ThreadList};
use crate::settings::SettingsView;
use crate::theme::{self, Palette};
use crate::update::{Status as UpdateStatus, Updater};
use crate::thread::{ThreadEvent, ThreadView, kbd};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Screen {
    View(View),
    Settings,
}

/// Height of the top bar, which doubles as the window's title bar.
const TITLE_H: f32 = 52.;
/// Room left of the title for the macOS window buttons.
const TRAFFIC_LIGHTS_W: f32 = 84.;

pub struct Yamc {
    core: Arc<Core>,
    focus: FocusHandle,
    accounts: Vec<Account>,
    mailboxes: Vec<Mailbox>,
    drafts: Vec<Draft>,
    screen: Screen,
    list: ThreadList,
    /// Bumped on every view change so late query results for an old view are dropped.
    generation: u64,
    open_row: Option<ThreadRow>,
    thread: Option<Entity<ThreadView>>,
    composer: Option<Entity<Composer>>,
    settings: Option<Entity<SettingsView>>,
    search: Entity<InputState>,
    expanded: HashSet<i64>,
    sidebar: bool,
    /// Mouse is down on a title-bar area; the next move drags the window.
    drag_armed: bool,
    help: bool,
    visual: bool,
    engine: KeyEngine,
    pending: String,
    key_timer: Option<Task<()>>,
    search_timer: Option<Task<()>>,
    toast: Option<String>,
    toast_timer: Option<Task<()>>,
    images: Images,
    updater: Entity<Updater>,
    theme_key: (String, String, bool),
    _subs: Vec<Subscription>,
}

impl Yamc {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search mail"));
        let sub = cx.subscribe_in(&search, window, |this: &mut Self, input, e: &InputEvent, window, cx| match e {
            // Results follow every keystroke; a short pause batches fast typing.
            InputEvent::Change => {
                let q = input.read(cx).value().trim().to_string();
                this.search_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(50)).await;
                    let _ = this.update(cx, |this, cx| this.live_search(q, cx));
                }));
            }
            InputEvent::PressEnter { .. } => {
                let q = input.read(cx).value().trim().to_string();
                this.search_timer = None;
                this.live_search(q, cx);
                window.focus(&this.focus, cx);
            }
            _ => {}
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        // Core events → UI refreshes.
        let mut rx = core.subscribe();
        cx.spawn(async move |this, cx| {
            loop {
                let e = match rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => CoreEvent::Threads,
                    Err(_) => break,
                };
                if this.update(cx, |this, cx| this.on_core_event(e, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Follow the OS light/dark switch when the theme is "system".
        let appearance = cx.observe_window_appearance(window, |this: &mut Self, window, cx| this.apply_theme(Some(window), cx));

        let updater = cx.new(|cx| Updater::new(core.runtime().clone(), cx));
        let updates = cx.observe(&updater, |_, _, cx| cx.notify());

        let s = core.get_settings();
        let mut this = Self {
            accounts: core.list_accounts(),
            mailboxes: core.list_mailboxes(),
            drafts: core.list_drafts(),
            core,
            focus,
            screen: Screen::View(View::Inbox),
            list: ThreadList::default(),
            generation: 0,
            open_row: None,
            thread: None,
            composer: None,
            settings: None,
            search,
            expanded: HashSet::new(),
            sidebar: true,
            drag_armed: false,
            help: false,
            visual: false,
            engine: KeyEngine::default(),
            pending: String::new(),
            key_timer: None,
            search_timer: None,
            toast: None,
            toast_timer: None,
            images: Images::default(),
            updater,
            theme_key: (s.theme, s.accent, false),
            _subs: vec![sub, appearance, updates],
        };
        this.apply_theme(Some(window), cx);
        if this.accounts.is_empty() {
            this.go(Screen::Settings, cx);
        } else {
            this.refresh(cx);
        }
        this
    }

    fn apply_theme(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let s = self.core.get_settings();
        let dark = theme::system_dark(window.as_deref(), cx);
        self.theme_key = (s.theme.clone(), s.accent.clone(), dark);
        theme::apply(theme::palette(&s.theme, &s.accent, dark), window, cx);
        cx.notify();
    }

    fn on_core_event(&mut self, e: CoreEvent, cx: &mut Context<Self>) {
        match e {
            CoreEvent::Threads => {
                self.mailboxes = self.core.list_mailboxes();
                self.refresh(cx);
                if let Some(t) = &self.thread {
                    t.update(cx, |t, cx| t.reload(false, cx));
                }
            }
            CoreEvent::Accounts => self.accounts = self.core.list_accounts(),
            CoreEvent::Drafts => self.drafts = self.core.list_drafts(),
            CoreEvent::Settings => {
                let s = self.core.get_settings();
                if (s.theme.clone(), s.accent.clone()) != (self.theme_key.0.clone(), self.theme_key.1.clone()) {
                    self.apply_theme(None, cx);
                }
            }
        }
        cx.notify();
    }

    fn current_view(&self) -> Option<View> {
        match &self.screen {
            Screen::View(v @ (View::Inbox | View::Sent | View::Mailbox(_) | View::Search(_))) => Some(v.clone()),
            _ => None,
        }
    }

    /// Re-query the list, merging so rows don't jump.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.current_view() else { return };
        let want = PAGE.max(self.list.rows.len() + 50);
        let generation = self.generation;
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            let rows = cx.background_spawn(async move { core.list_threads(&view, 0, want) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.list.apply(rows, want);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.list.loading_more || self.list.exhausted || self.list.stale {
            return;
        }
        let Some(view) = self.current_view() else { return };
        self.list.loading_more = true;
        let offset = self.list.rows.len();
        let generation = self.generation;
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            let rows = cx.background_spawn(async move { core.list_threads(&view, offset, PAGE) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.list.append(rows);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn go(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.close_thread(cx);
        self.composer = None;
        if screen == self.screen {
            cx.notify();
            return;
        }
        let refine = matches!((&self.screen, &screen), (Screen::View(View::Search(_)), Screen::View(View::Search(_))));
        self.screen = screen;
        self.generation += 1;
        if refine {
            self.list.reset_keep_rows();
        } else {
            self.list.reset();
        }
        self.visual = false;
        if self.screen != Screen::Settings {
            self.settings = None;
        }
        self.refresh(cx);
        cx.notify();
    }

    /// Show results for the search box's text; an empty box goes back to the inbox.
    fn live_search(&mut self, q: String, cx: &mut Context<Self>) {
        if q.is_empty() {
            if matches!(self.screen, Screen::View(View::Search(_))) {
                self.go(Screen::View(View::Inbox), cx);
            }
        } else {
            self.go(Screen::View(View::Search(q)), cx);
        }
    }

    fn close_thread(&mut self, cx: &mut Context<Self>) {
        self.open_row = None;
        self.thread = None;
        cx.notify();
    }

    fn open(&mut self, row: ThreadRow, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        let images = self.images.clone();
        let (a, t) = (row.account_id, row.thread_id.clone());
        let view = cx.new(|cx| ThreadView::new(core, a, t, images, cx));
        let sub = cx.subscribe_in(&view, window, |this: &mut Self, _, e: &ThreadEvent, window, cx| match e {
            ThreadEvent::Reply(id, all) => this.reply(*id, *all, window, cx),
        });
        self._subs.push(sub);
        self.thread = Some(view);
        // After the view has read which messages were unread (they start expanded).
        if row.unread {
            self.list.mark_local(&row.key(), false);
            let _ = self.core.mark_thread(row.account_id, &row.thread_id, true);
        }
        self.open_row = Some(row);
        cx.notify();
    }

    fn notify_toast(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        let msg = msg.into();
        // Errors run long; leave them up long enough to read.
        let secs = if msg.len() > 40 { 12 } else { 4 };
        self.toast = Some(msg);
        self.toast_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(secs)).await;
            let _ = this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn archive(&mut self, row: &ThreadRow, cx: &mut Context<Self>) {
        if self.screen == Screen::View(View::Sent) {
            return self.notify_toast("Archive moves mail out of the Inbox; use Delete in Sent", cx);
        }
        self.list.remove_local(&row.key());
        if let Err(e) = self.core.archive_thread(row.account_id, &row.thread_id) {
            self.notify_toast(format!("{e:#}"), cx);
            self.refresh(cx);
        }
    }

    fn trash(&mut self, row: &ThreadRow, cx: &mut Context<Self>) {
        self.list.remove_local(&row.key());
        if let Err(e) = self.core.trash_thread(row.account_id, &row.thread_id) {
            self.notify_toast(format!("{e:#}"), cx);
            self.refresh(cx);
        }
    }

    fn targets(&mut self) -> Vec<ThreadRow> {
        if let Some(r) = &self.open_row {
            return vec![r.clone()];
        }
        let t = self.list.targets();
        self.list.clear_selection();
        self.visual = false;
        t
    }

    fn mark_rows(&mut self, unread: bool, cx: &mut Context<Self>) {
        for r in self.targets() {
            self.list.mark_local(&r.key(), unread);
            let _ = self.core.mark_thread(r.account_id, &r.thread_id, !unread);
        }
        cx.notify();
    }

    fn reply(&mut self, message_id: i64, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.core.reply_draft(message_id, all) {
            Ok(d) => self.compose(d, window, cx),
            Err(e) => self.notify_toast(format!("{e:#}"), cx),
        }
    }

    fn reply_to_open(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.thread.as_ref().and_then(|t| t.read(cx).last_message());
        if let Some(id) = last {
            self.reply(id, all, window, cx);
        }
    }

    fn new_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(first) = self.accounts.first() else {
            return self.go(Screen::Settings, cx);
        };
        let account_id = self.open_row.as_ref().map(|r| r.account_id).unwrap_or(first.id);
        self.compose(DraftInput { account_id, ..Default::default() }, window, cx);
    }

    fn compose(&mut self, d: DraftInput, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        let accounts = self.accounts.clone();
        let c = cx.new(|cx| Composer::new(core, d, accounts, window, cx));
        let sub = cx.subscribe_in(&c, window, |this: &mut Self, _, e: &ComposerEvent, window, cx| {
            this.composer = None;
            if let ComposerEvent::Send(d) = e {
                this.send_in_background(d.clone(), cx);
            }
            window.focus(&this.focus, cx);
            cx.notify();
        });
        self._subs.push(sub);
        self.composer = Some(c);
        cx.notify();
    }

    fn send_in_background(&mut self, d: DraftInput, cx: &mut Context<Self>) {
        self.notify_toast("Sending…", cx);
        let core = self.core.clone();
        let task = self.core.runtime().spawn(async move { core.send(&d).await });
        cx.spawn(async move |this, cx| {
            let r = task.await.map_err(anyhow::Error::from).and_then(|r| r);
            let _ = this.update(cx, |this, cx| match r {
                Ok(()) => this.notify_toast("Sent", cx),
                Err(e) => this.notify_toast(format!("Not sent, it's in Drafts: {e:#}"), cx),
            });
        })
        .detach();
    }

    fn step_thread(&mut self, d: isize, window: &mut Window, cx: &mut Context<Self>) {
        let cur = self.open_row.as_ref().map(ThreadRow::key);
        if let Some(next) = self.list.move_by(d).cloned()
            && Some(next.key()) != cur
        {
            self.open(next, window, cx);
        }
    }

    fn mode(&self) -> Mode {
        if self.open_row.is_some() {
            Mode::Thread
        } else if self.current_view().is_some() {
            Mode::List
        } else {
            Mode::Page
        }
    }

    fn page_scroll(&mut self, dy: f32, cx: &mut Context<Self>) {
        if let Some(t) = &self.thread {
            t.update(cx, |t, cx| t.scroll_by(dy, cx));
        } else if let Some(s) = &self.settings {
            s.update(cx, |s, cx| {
                let h = &s.scroll;
                let o = h.offset();
                let y = (o.y - px(dy)).max(-h.max_offset().y).min(px(0.));
                h.set_offset(point(o.x, y));
                cx.notify();
            });
        }
    }

    fn page_height(&self, cx: &App) -> f32 {
        self.thread.as_ref().map(|t| t.read(cx).page_height()).unwrap_or(600.)
    }

    fn run(&mut self, cmd: Cmd, count: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let n = count.unwrap_or(1) as isize;
        let nf = count.unwrap_or(1) as f32;
        match cmd {
            Cmd::Down => drop(self.list.move_by(n)),
            Cmd::Up => drop(self.list.move_by(-n)),
            Cmd::First => drop(self.list.move_to(count.map(|c| c as isize - 1).unwrap_or(0))),
            Cmd::Last => drop(self.list.move_to(count.map(|c| c as isize - 1).unwrap_or(-1))),
            Cmd::ScreenTop => self.list.screen(ScreenPos::Top),
            Cmd::ScreenMiddle => self.list.screen(ScreenPos::Middle),
            Cmd::ScreenBottom => self.list.screen(ScreenPos::Bottom),
            Cmd::HalfDown => self.list.page(0.5 * nf),
            Cmd::HalfUp => self.list.page(-0.5 * nf),
            Cmd::PageDown => self.list.page(nf),
            Cmd::PageUp => self.list.page(-nf),
            Cmd::AlignCenter => self.list.align(ScrollStrategy::Center),
            Cmd::AlignTop => self.list.align(ScrollStrategy::Top),
            Cmd::AlignBottom => self.list.align(ScrollStrategy::Bottom),
            Cmd::ToggleSelect => self.list.toggle_select(),
            Cmd::Visual => self.visual = self.list.visual(),
            Cmd::ClearOrInbox => {
                let had = self.list.clear_selection();
                self.visual = false;
                if !had && self.screen != Screen::View(View::Inbox) {
                    self.search.update(cx, |s, cx| s.set_value("", window, cx));
                    self.go(Screen::View(View::Inbox), cx);
                }
            }
            Cmd::Open => {
                if let Some(r) = self.list.current().cloned() {
                    self.open(r, window, cx);
                }
            }
            Cmd::Archive => {
                for r in self.targets() {
                    self.archive(&r, cx);
                }
                self.close_thread(cx);
            }
            Cmd::Trash => {
                let rows = self.targets();
                for r in &rows {
                    self.trash(r, cx);
                }
                if rows.len() > 1 {
                    self.notify_toast(format!("Moved {} conversations to Trash", rows.len()), cx);
                }
                self.close_thread(cx);
            }
            Cmd::MarkUnread => {
                self.mark_rows(true, cx);
                self.close_thread(cx);
            }
            Cmd::MarkRead => self.mark_rows(false, cx),
            Cmd::Reply => self.reply_to_open(false, window, cx),
            Cmd::ReplyAll => self.reply_to_open(true, window, cx),
            Cmd::Compose => self.new_message(window, cx),
            Cmd::ScrollDown => self.page_scroll(60. * nf, cx),
            Cmd::ScrollUp => self.page_scroll(-60. * nf, cx),
            Cmd::ScrollHalfDown => self.page_scroll(self.page_height(cx) / 2. * nf, cx),
            Cmd::ScrollHalfUp => self.page_scroll(-self.page_height(cx) / 2. * nf, cx),
            Cmd::ScrollPageDown => self.page_scroll(self.page_height(cx) * 0.9 * nf, cx),
            Cmd::ScrollPageUp => self.page_scroll(-self.page_height(cx) * 0.9 * nf, cx),
            Cmd::ScrollTop => self.page_scroll(-1e7, cx),
            Cmd::ScrollBottom => self.page_scroll(1e7, cx),
            Cmd::NextThread => self.step_thread(n, window, cx),
            Cmd::PrevThread => self.step_thread(-n, window, cx),
            Cmd::LoadImages => {
                if let Some(t) = &self.thread {
                    t.update(cx, |t, cx| t.load_images(cx));
                }
            }
            Cmd::OpenOriginal => self.open_original(cx),
            Cmd::Back => self.close_thread(cx),
            Cmd::GoInbox => self.go(Screen::View(View::Inbox), cx),
            Cmd::GoDrafts => self.go(Screen::View(View::Drafts), cx),
            Cmd::GoSent => self.go(Screen::View(View::Sent), cx),
            Cmd::GoSettings => self.go(Screen::Settings, cx),
            Cmd::Search => {
                let f = self.search.read(cx).focus_handle(cx);
                window.focus(&f, cx);
                self.search.update(cx, |s, cx| s.select_all(window, cx));
            }
            Cmd::ToggleSidebar => self.sidebar = !self.sidebar,
            Cmd::Help => self.help = true,
            Cmd::BackToInbox => self.go(Screen::View(View::Inbox), cx),
        }
        // The list asks for more as the cursor nears the end.
        if self.list.cursor + 40 >= self.list.rows.len() {
            self.load_more(cx);
        }
        cx.notify();
    }

    /// Write the newest open message to a temp file and hand it to the browser.
    fn open_original(&mut self, cx: &mut Context<Self>) {
        let Some(html) = self.thread.as_ref().and_then(|t| t.read(cx).original_html()) else { return };
        let path = std::env::temp_dir().join(format!("yamc-{}.html", yamc_core::store::now_ms()));
        let doc = format!(
            "<!doctype html><meta charset=utf-8><meta http-equiv=\"Content-Security-Policy\" content=\"script-src 'none'\">{html}"
        );
        match std::fs::write(&path, doc) {
            Ok(()) => cx.open_url(&format!("file://{}", path.display())),
            Err(e) => self.notify_toast(format!("{e}"), cx),
        }
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if self.help {
            if matches!(k.key.as_str(), "escape" | "q") || k.key_char.as_deref() == Some("?") {
                self.help = false;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.composer.is_some() {
            return;
        }
        // Typing in the search box (or any other input).
        let typing = window.context_stack().iter().any(|c| c.contains("Input"));
        if typing {
            let search_focused = self.search.read(cx).focus_handle(cx).is_focused(window);
            if matches!(k.key.as_str(), "down" | "tab") && search_focused {
                window.focus(&self.focus, cx);
                cx.stop_propagation();
                return;
            }
            if k.key == "escape" && search_focused {
                window.focus(&self.focus, cx);
                if matches!(self.screen, Screen::View(View::Search(_))) {
                    self.search.update(cx, |s, cx| s.set_value("", window, cx));
                    self.go(Screen::View(View::Inbox), cx);
                }
                cx.stop_propagation();
            }
            return;
        }
        let m = &k.modifiers;
        let Some(tok) = key_token(&k.key, k.key_char.as_deref(), m.control, m.alt, m.platform) else { return };
        self.key_timer = None;
        match self.engine.feed(&tok, self.mode()) {
            Feed::Run(cmd, count) => {
                cx.stop_propagation();
                self.pending.clear();
                self.run(cmd, count, window, cx);
            }
            Feed::Pending(p) => {
                cx.stop_propagation();
                self.pending = p;
                self.key_timer = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(1000)).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.pending.clear();
                        if let Feed::Run(cmd, count) = this.engine.flush() {
                            this.run(cmd, count, window, cx);
                        }
                        cx.notify();
                    });
                }));
                cx.notify();
            }
            Feed::None => {
                if !self.pending.is_empty() {
                    self.pending.clear();
                    cx.notify();
                }
            }
        }
    }

    // ---------- rendering ----------

    fn title(&self) -> String {
        match &self.screen {
            Screen::Settings => "Settings".into(),
            Screen::View(View::Inbox) => "Inbox".into(),
            Screen::View(View::Sent) => "Sent".into(),
            Screen::View(View::Drafts) => "Drafts".into(),
            Screen::View(View::Search(q)) => format!("Search: {q}"),
            Screen::View(View::Mailbox(id)) => self.mailboxes.iter().find(|m| m.id == *id).map(|m| m.name.clone()).unwrap_or_default(),
        }
    }

    fn nav(&self, id: impl Into<ElementId>, icon: Option<IconName>, label: String, on: bool, pal: &Palette) -> Stateful<Div> {
        h_flex()
            .id(id)
            .gap_2()
            .h(px(30.))
            .px_2()
            .mx_2()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .text_color(if on { pal.strong } else { pal.text })
            .when(on, |d| d.bg(pal.active))
            .when(!on, |d| d.hover(|s| s.bg(pal.hover)))
            .when_some(icon, |d, i| d.child(Icon::new(i).size_4().text_color(if on { pal.strong } else { pal.muted })))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
    }

    /// Makes an element act as the window's title bar: drag to move, double-click to zoom.
    fn title_drag(&self, d: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        d.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, e: &MouseDownEvent, window, _| {
                if e.click_count >= 2 {
                    this.drag_armed = false;
                    window.titlebar_double_click();
                } else {
                    this.drag_armed = true;
                }
            }),
        )
        .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag_armed = false))
        .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, _| {
            if this.drag_armed && e.pressed_button == Some(MouseButton::Left) {
                this.drag_armed = false;
                window.start_window_move();
            } else if e.pressed_button.is_none() {
                this.drag_armed = false;
            }
        }))
    }

    fn render_sidebar(&self, pal: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.title_drag(h_flex().id("sidebar-title").h(px(TITLE_H)).flex_none().pl(px(TRAFFIC_LIGHTS_W)).pr_3().gap_2(), cx);
        let compose = h_flex()
            .id("compose")
            .mx_2()
            .mb_2()
            .h(px(32.))
            .px_2()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(pal.line)
            .bg(pal.card)
            .text_sm()
            .text_color(pal.strong)
            .cursor_pointer()
            .hover(|s| s.bg(pal.hover))
            .on_click(cx.listener(|this, _, window, cx| this.new_message(window, cx)))
            .child(Icon::new(IconName::SquarePen).size_4().text_color(pal.muted))
            .child(div().flex_1().child("New message"))
            .child(kbd("c", pal));
        let mut col = v_flex()
            .w(px(232.))
            .flex_none()
            .h_full()
            .bg(pal.panel)
            .border_r_1()
            .border_color(pal.line)
            .pb_3()
            .gap_0p5()
            .child(
                header
                    .child(
                        div()
                            .size(px(22.))
                            .rounded_md()
                            .bg(pal.accent)
                            .text_color(pal.accent_fg)
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("y"),
                    )
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(pal.strong).child("yamc")),
            )
            .child(compose)
            .child(
                self.nav("nav-inbox", Some(IconName::Inbox), "Inbox".into(), self.screen == Screen::View(View::Inbox), pal)
                    .on_click(cx.listener(|this, _, _, cx| this.go(Screen::View(View::Inbox), cx))),
            )
            .child(
                self.nav("nav-sent", Some(IconName::Send), "Sent".into(), self.screen == Screen::View(View::Sent), pal)
                    .on_click(cx.listener(|this, _, _, cx| this.go(Screen::View(View::Sent), cx))),
            )
            .child(
                self.nav("nav-drafts", Some(IconName::FilePen), "Drafts".into(), self.screen == Screen::View(View::Drafts), pal)
                    .on_click(cx.listener(|this, _, _, cx| this.go(Screen::View(View::Drafts), cx))),
            );
        if !self.accounts.is_empty() {
            col = col.child(div().px_4().pt_4().pb_1().text_xs().text_color(pal.faint).child("Accounts"));
        }
        for (i, a) in self.accounts.iter().enumerate() {
            let id = a.id;
            let expanded = self.expanded.contains(&id);
            let bad = matches!(a.state, yamc_core::types::SyncState::Error | yamc_core::types::SyncState::Offline);
            let dot = div().size(px(7.)).rounded_full().bg(if bad { pal.faint } else { pal.account(i) });
            col = col.child(
                h_flex()
                    .id(("acct", id as u64))
                    .gap_2()
                    .h(px(30.))
                    .px_2()
                    .mx_2()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(pal.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&id) {
                            this.expanded.insert(id);
                        }
                        cx.notify();
                    }))
                    .child(div().w(px(16.)).flex().justify_center().child(dot))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(if a.name.is_empty() {
                        a.email.clone()
                    } else {
                        a.name.clone()
                    }))
                    .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size_3().text_color(pal.faint)),
            );
            if expanded {
                for m in self.mailboxes.iter().filter(|m| m.account_id == id) {
                    let mid = m.id;
                    let on = self.screen == Screen::View(View::Mailbox(mid));
                    col = col.child(
                        self.nav(("mbox", mid as u64), None, m.name.clone(), on, pal)
                            .pl(px(34. + m.depth as f32 * 12.))
                            .on_click(cx.listener(move |this, _, _, cx| this.go(Screen::View(View::Mailbox(mid)), cx))),
                    );
                }
                if let Some(e) = &a.error {
                    col = col.child(div().px_4().py_1().text_xs().text_color(pal.danger).child(e.clone()));
                }
            }
        }
        col.child(div().flex_1()).children(self.render_update(pal, cx)).child(
            self.nav("nav-settings", Some(IconName::Settings), "Settings".into(), self.screen == Screen::Settings, pal)
                .on_click(cx.listener(|this, _, _, cx| this.go(Screen::Settings, cx))),
        )
    }

    /// Sidebar line for an update in progress, ready, or a check you asked for.
    fn render_update(&self, pal: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.updater.read(cx).status.clone();
        let line = |text: String, color: Hsla| div().px_4().py_1().text_xs().text_color(color).child(text).into_any_element();
        Some(match status {
            UpdateStatus::Idle | UpdateStatus::Checking => return None,
            UpdateStatus::UpToDate => line(format!("yamc {} is up to date", crate::update::VERSION), pal.faint),
            UpdateStatus::Downloading(v) => line(format!("Downloading yamc {v}…"), pal.faint),
            UpdateStatus::Failed(e) => line(e, pal.danger),
            UpdateStatus::Ready(v) => h_flex()
                .id("update-restart")
                .mx_2()
                .mb_1()
                .px_2()
                .h(px(30.))
                .gap_2()
                .rounded_md()
                .bg(pal.accent)
                .text_color(pal.accent_fg)
                .text_sm()
                .cursor_pointer()
                .child(Icon::new(IconName::Download).size_4())
                .child(format!("Restart to update to {v}"))
                .on_click(cx.listener(|this, _, _, cx| this.updater.update(cx, |u, cx| u.restart(cx))))
                .into_any_element(),
        })
    }

    fn icon_button(&self, id: &'static str, icon: IconName, label: &'static str, key: &'static str, pal: &Palette) -> Stateful<Div> {
        h_flex()
            .id(id)
            .h(px(28.))
            .px_2()
            .gap_1p5()
            .rounded_md()
            .text_sm()
            .text_color(pal.muted)
            .cursor_pointer()
            .hover(|s| s.bg(pal.hover).text_color(pal.strong))
            .child(Icon::new(icon).size_4())
            .child(label)
            .child(kbd(key, pal))
    }

    fn render_topbar(&self, pal: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title();
        let title_el = if self.open_row.is_some() {
            h_flex()
                .id("back")
                .flex_none()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .gap_1()
                .text_sm()
                .text_color(pal.muted)
                .cursor_pointer()
                .hover(|s| s.text_color(pal.strong))
                .on_click(cx.listener(|this, _, _, cx| this.close_thread(cx)))
                .child(Icon::new(IconName::ArrowLeft).size_4())
                .child(title)
                .into_any_element()
        } else {
            div().flex_none().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(pal.strong).child(title).into_any_element()
        };
        let actions = self.open_row.is_some().then(|| {
            h_flex()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    self.icon_button("act-archive", IconName::Archive, "Archive", "e", pal)
                        .on_click(cx.listener(|this, _, window, cx| this.run(Cmd::Archive, None, window, cx))),
                )
                .child(
                    self.icon_button("act-trash", IconName::Trash, "Delete", "#", pal)
                        .on_click(cx.listener(|this, _, window, cx| this.run(Cmd::Trash, None, window, cx))),
                )
                .child(
                    self.icon_button("act-unread", IconName::Mail, "Unread", "U", pal)
                        .on_click(cx.listener(|this, _, window, cx| this.run(Cmd::MarkUnread, None, window, cx))),
                )
        });
        self.title_drag(
            h_flex()
                .id("topbar")
                .h(px(TITLE_H))
                .flex_none()
                .pr_5()
                .pl(px(if self.sidebar { 20. } else { TRAFFIC_LIGHTS_W }))
                .gap_4()
                .border_b_1()
                .border_color(pal.line),
            cx,
        )
        .child(title_el)
        .children(actions)
        .child(div().flex_1())
        .child(
            h_flex()
                .w(px(320.))
                .gap_2()
                // Selecting text in the box must not drag the window.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().flex_1().child(Input::new(&self.search).small().prefix(Icon::new(IconName::Search).size_4().text_color(pal.faint))))
                .child(kbd("/", pal)),
        )
    }

    fn render_row(&self, ix: usize, pal: &Palette, multi: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = self.list.rows.get(ix) else { return div().into_any_element() };
        let cursor = ix == self.list.cursor;
        let selected = self.list.is_selected(ix);
        let color = self.accounts.iter().position(|a| a.id == r.account_id).map(|i| pal.account(i)).unwrap_or(pal.faint);
        let row = r.clone();
        let inner = h_flex()
            .id(("row", ix))
            .w_full()
            .h(px(ROW))
            .px_3()
            .gap_4()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .group("row")
            .when(cursor, |d| d.bg(pal.active))
            .when(!cursor, |d| d.hover(|s| s.bg(pal.hover)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.list.cursor = ix;
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.open(row.clone(), window, cx)))
            .child(
                h_flex()
                    .w(px(210.))
                    .flex_none()
                    .gap_2()
                    .when(multi, |d| d.child(div().size(px(6.)).flex_none().rounded_full().bg(color)))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if r.unread { pal.strong } else { pal.muted })
                            .when(r.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(r.from.clone()),
                    )
                    .when(r.count > 1, |d| d.child(div().flex_none().text_xs().text_color(pal.faint).child(r.count.to_string()))),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.7))
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if r.unread { pal.strong } else { pal.text })
                            .when(r.unread, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(r.subject.clone()),
                    )
                    .when(!r.snippet.is_empty(), |d| {
                        d.child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(pal.faint).child(r.snippet.clone()))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(72.))
                    .text_right()
                    .text_xs()
                    .text_color(if r.unread { pal.text } else { pal.faint })
                    .group_hover("row", |s| s.invisible())
                    .child(short_date(r.date)),
            )
            .child({
                let (a, t) = (r.clone(), r.clone());
                let act = |id: (&'static str, usize), icon: IconName, pal: &Palette| {
                    div()
                        .id(id)
                        .size(px(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .text_color(pal.muted)
                        .hover(|s| s.bg(pal.line).text_color(pal.strong))
                        .child(Icon::new(icon).size_4())
                };
                h_flex()
                    .absolute()
                    .right_2()
                    .top_0()
                    .h_full()
                    .gap_0p5()
                    .invisible()
                    .group_hover("row", |s| s.visible())
                    .child(act(("row-archive", ix), IconName::Archive, pal).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.archive(&a, cx);
                        cx.notify();
                    })))
                    .child(act(("row-trash", ix), IconName::Trash, pal).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.trash(&t, cx);
                        cx.notify();
                    })))
            });
        // Selection is a flat bar in the gutter; consecutive rows join into one line.
        div()
            .w_full()
            .px_2()
            .relative()
            .when(selected, |d| d.child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(pal.accent)))
            .child(inner.relative())
            .into_any_element()
    }

    fn empty(icon: IconName, title: &'static str, body: &'static str, pal: &Palette) -> AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Icon::new(icon).size_6().text_color(pal.faint))
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).text_color(pal.strong).child(title))
            .child(div().text_sm().text_color(pal.muted).child(body))
            .into_any_element()
    }

    fn render_list(&self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.list.loaded && self.list.rows.is_empty() {
            let search = matches!(self.screen, Screen::View(View::Search(_)));
            return if search {
                Self::empty(IconName::SearchX, "Nothing found", "Try fewer words.", pal)
            } else if self.accounts.is_empty() {
                Self::empty(IconName::Inbox, "No accounts yet", "Add one in Settings (g s).", pal)
            } else {
                match &self.screen {
                    Screen::View(View::Sent) => Self::empty(IconName::Send, "Nothing sent yet", "Mail you send shows up here.", pal),
                    _ => Self::empty(IconName::Inbox, "All clear", "Nothing in here.", pal),
                }
            };
        }
        let multi = self.accounts.len() > 1;
        let pal = pal.clone();
        uniform_list(
            "threads",
            self.list.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                if range.end + 40 >= this.list.rows.len() {
                    this.load_more(cx);
                }
                range.map(|ix| this.render_row(ix, &pal, multi, cx)).collect::<Vec<_>>()
            }),
        )
        .py_2()
        .size_full()
        .track_scroll(&self.list.scroll)
        .into_any_element()
    }

    fn render_drafts(&self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.drafts.is_empty() {
            return Self::empty(IconName::FilePen, "No drafts", "Drafts you keep, and ones agents write, show up here.", pal);
        }
        div()
            .id("drafts")
            .size_full()
            .overflow_y_scroll()
            .py_2()
            .children(self.drafts.iter().map(|d| {
                let draft = d.clone();
                h_flex()
                    .id(("draft", d.id as u64))
                    .h(px(ROW))
                    .mx_2()
                    .min_w_0()
                    .px_3()
                    .gap_4()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(pal.hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.compose(DraftInput::from(&draft), window, cx)))
                    .child(
                        div()
                            .w(px(210.))
                            .flex_none()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(pal.strong)
                            .child(if d.to.is_empty() { "(no recipient)".to_string() } else { d.to.clone() }),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(div().flex_none().text_color(pal.text).child(if d.subject.is_empty() { "(no subject)".into() } else { d.subject.clone() }))
                            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(pal.faint).child(d.body.trim().replace('\n', " "))),
                    )
                    .when(d.created_by == "agent", |r| {
                        r.child(div().flex_none().px_2().rounded_md().border_1().border_color(pal.line).text_xs().text_color(pal.muted).child("agent"))
                    })
                    .child(div().flex_none().w(px(72.)).text_right().text_xs().text_color(pal.faint).child(short_date(d.updated_at)))
            }))
            .into_any_element()
    }

    fn render_help(&self, pal: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let order = ["Move", "Scroll", "Select", "Act", "Reading", "Go"];
        let reading_extra: &[(&[&str], &str)] =
            &[(&["j", "k"], "Scroll"), (&["<C-d>", "<C-u>"], "Half page"), (&["gg", "G"], "Top / bottom"), (&["e"], "Archive")];
        let group = |g: &'static str| {
            let mut items: Vec<(Vec<String>, String)> = Vec::new();
            if g == "Reading" {
                items.extend(reading_extra.iter().map(|(k, d)| (k.iter().map(|k| pretty(k)).collect(), d.to_string())));
            }
            items.extend(
                BINDINGS
                    .iter()
                    .filter(|b| b.group == g && !b.hidden)
                    .map(|b| (b.keys.iter().take(3).map(|k| pretty(k)).collect(), b.desc.to_string())),
            );
            v_flex()
                .w(px(250.))
                .gap_1()
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(pal.strong).pb_1().child(if g == "Reading" {
                    "Reading a conversation"
                } else {
                    g
                }))
                .children(items.into_iter().map(|(keys, desc)| {
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(h_flex().w(px(84.)).flex_none().gap_1().children(keys.iter().map(|k| kbd(k, pal))))
                        .child(div().text_color(pal.muted).child(desc))
                }))
        };
        div()
            .id("help-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(Hsla { a: 0.55, ..black() })
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this, _, _, cx| {
                this.help = false;
                cx.notify();
            }))
            .child(
                v_flex()
                    .id("help")
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .w(px(820.))
                    .max_h(relative(0.85))
                    .overflow_y_scroll()
                    .p_6()
                    .gap_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(pal.line)
                    .bg(pal.card)
                    .shadow_lg()
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).text_color(pal.strong).child("Keyboard"))
                    .child(div().text_sm().text_color(pal.muted).child("Vim-style. Prefix a count to repeat (5j, 3J). esc to close."))
                    .child(h_flex().flex_wrap().items_start().gap_x_6().gap_y_5().children(order.iter().map(|g| group(g)))),
            )
    }
}

impl Render for Yamc {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = cx.global::<Palette>().clone();
        // Focus left with a closed form or composer: take it back so keys keep working.
        if window.focused(cx).is_none() {
            cx.defer_in(window, |this, window, cx| window.focus(&this.focus, cx));
        }
        if self.screen == Screen::Settings && self.settings.is_none() {
            let core = self.core.clone();
            let adding = self.accounts.is_empty();
            self.settings = Some(cx.new(|cx| SettingsView::new(core, adding, window, cx)));
        }
        let content: AnyElement = if let Some(t) = &self.thread {
            t.clone().into_any_element()
        } else {
            match &self.screen {
                Screen::Settings => self.settings.clone().map(|s| s.into_any_element()).unwrap_or_else(|| div().into_any_element()),
                Screen::View(View::Drafts) => self.render_drafts(&pal, cx),
                Screen::View(_) => self.render_list(&pal, cx),
            }
        };
        let status = (!self.pending.is_empty() || (self.visual && self.open_row.is_none())).then(|| {
            h_flex()
                .absolute()
                .bottom_3()
                .right_4()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(pal.line)
                .bg(pal.card)
                .text_xs()
                .font_family("monospace")
                .when(self.visual && self.open_row.is_none(), |d| d.child(div().text_color(pal.accent).font_weight(FontWeight::BOLD).child("VISUAL")))
                .when(!self.pending.is_empty(), |d| d.child(div().text_color(pal.strong).child(self.pending.clone())))
        });
        div()
            .id("yamc")
            .key_context("Yamc")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(|this, _: &menus::CheckForUpdates, _, cx| this.updater.update(cx, |u, cx| u.check(true, cx))))
            .on_action(cx.listener(|this, _: &menus::NewMessage, w, cx| this.run(Cmd::Compose, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::OpenSettings, w, cx| this.run(Cmd::GoSettings, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::FocusSearch, w, cx| this.run(Cmd::Search, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::GoInbox, w, cx| this.run(Cmd::GoInbox, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::GoDrafts, w, cx| this.run(Cmd::GoDrafts, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::GoSent, w, cx| this.run(Cmd::GoSent, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::Archive, w, cx| this.run(Cmd::Archive, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::Trash, w, cx| this.run(Cmd::Trash, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::MarkUnread, w, cx| this.run(Cmd::MarkUnread, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::MarkRead, w, cx| this.run(Cmd::MarkRead, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::Reply, w, cx| this.run(Cmd::Reply, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::ReplyAll, w, cx| this.run(Cmd::ReplyAll, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::ToggleSidebar, w, cx| this.run(Cmd::ToggleSidebar, None, w, cx)))
            .on_action(cx.listener(|this, _: &menus::ShowKeys, w, cx| this.run(Cmd::Help, None, w, cx)))
            .on_action(|_: &menus::Minimize, w, _| w.minimize_window())
            .on_action(|_: &menus::Zoom, w, _| w.zoom_window())
            .size_full()
            .relative()
            .bg(pal.bg)
            .text_color(pal.text)
            .child(
                h_flex()
                    .size_full()
                    .when(self.sidebar, |d| d.child(self.render_sidebar(&pal, cx)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .relative()
                            .child(self.render_topbar(&pal, cx))
                            .child(div().flex_1().min_h_0().relative().child(content))
                            .children(status),
                    ),
            )
            // Overlays occlude: without it a click on them also lands on the list row underneath.
            .when_some(self.composer.clone(), |d, c| d.child(div().absolute().bottom_4().right_4().occlude().child(c)))
            .when_some(self.toast.clone(), |d, t| {
                d.child(
                    div()
                        .absolute()
                        .bottom_4()
                        .left(px(if self.sidebar { 248. } else { 16. }))
                        .occlude()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(pal.line)
                        .bg(pal.card)
                        .text_sm()
                        .text_color(pal.strong)
                        .child(t),
                )
            })
            .when(self.help, |d| d.child(self.render_help(&pal, cx)))
    }
}
