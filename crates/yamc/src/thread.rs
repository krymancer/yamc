//! A conversation: messages stacked Gmail-style, latest and unread expanded.

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Icon, Sizable, h_flex, v_flex};
use gpui_kit::assets::IconName;
use gpui_kit::*;
use gpui_kit::prelude::*;
use yamc_core::core::Core;
use yamc_core::types::{Message, Thread};

use crate::body::{Images, has_remote_images, prepare_html, text_to_html};
use crate::format::{long_date, short_date};
use crate::theme::Palette;

pub enum ThreadEvent {
    Reply(i64, bool),
}

impl EventEmitter<ThreadEvent> for ThreadView {}

pub struct ThreadView {
    core: Arc<Core>,
    pub account_id: i64,
    pub thread_id: String,
    thread: Option<Thread>,
    open: HashSet<i64>,
    loading: HashSet<i64>,
    /// Messages whose remote images are allowed.
    images_on: HashSet<i64>,
    images: Images,
    pub scroll: ScrollHandle,
}

fn initials(m: &Message) -> String {
    let src = m.from.as_ref().map(|a| a.display().to_string()).unwrap_or_else(|| "?".into());
    let clean: String = src.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect();
    let parts: Vec<&str> = clean.split_whitespace().collect();
    let first = parts.first().and_then(|p| p.chars().next()).unwrap_or('?');
    let last = if parts.len() > 1 { parts.last().and_then(|p| p.chars().next()) } else { None };
    format!("{first}{}", last.map(String::from).unwrap_or_default()).to_uppercase()
}

fn addr_list(list: &[yamc_core::types::Address]) -> String {
    list.iter().map(|a| a.formatted()).collect::<Vec<_>>().join(", ")
}

impl ThreadView {
    pub fn new(core: Arc<Core>, account_id: i64, thread_id: String, images: Images, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            core,
            account_id,
            thread_id,
            thread: None,
            open: HashSet::new(),
            loading: HashSet::new(),
            images_on: HashSet::new(),
            images,
            scroll: ScrollHandle::new(),
        };
        this.reload(true, cx);
        this
    }

    pub fn reload(&mut self, first: bool, cx: &mut Context<Self>) {
        let t = self.core.get_thread(self.account_id, &self.thread_id);
        if first {
            // Latest message plus anything unread starts expanded.
            let n = t.messages.len();
            self.open = t.messages.iter().enumerate().filter(|(i, m)| !m.seen || *i + 1 == n).map(|(_, m)| m.id).collect();
            let senders = self.core.get_settings().image_senders;
            for m in &t.messages {
                if m.from.as_ref().is_some_and(|f| senders.contains(&f.address.to_lowercase())) {
                    self.images_on.insert(m.id);
                }
            }
        }
        self.thread = Some(t);
        self.load_bodies(cx);
        self.fetch_images(cx);
        cx.notify();
    }

    fn load_bodies(&mut self, cx: &mut Context<Self>) {
        let Some(t) = &self.thread else { return };
        let need: Vec<i64> =
            t.messages.iter().filter(|m| self.open.contains(&m.id) && !m.has_body && !self.loading.contains(&m.id)).map(|m| m.id).collect();
        for id in need {
            self.loading.insert(id);
            let core = self.core.clone();
            let task = self.core.runtime().spawn(async move { core.load_message(id).await });
            cx.spawn(async move |this, cx| {
                let loaded = task.await.ok().flatten();
                let _ = this.update(cx, |this, cx| {
                    this.loading.remove(&id);
                    if let (Some(m), Some(t)) = (loaded, this.thread.as_mut())
                        && let Some(slot) = t.messages.iter_mut().find(|x| x.id == id)
                    {
                        *slot = m;
                    }
                    this.fetch_images(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn fetch_images(&mut self, cx: &mut Context<Self>) {
        let Some(t) = &self.thread else { return };
        let urls: Vec<String> = t
            .messages
            .iter()
            .filter(|m| self.images_on.contains(&m.id) && self.open.contains(&m.id))
            .filter_map(|m| m.html.as_deref())
            .flat_map(|h| self.images.missing(h))
            .collect();
        if urls.is_empty() {
            return;
        }
        let images = self.images.clone();
        let task = self.core.runtime().spawn(async move { images.fetch(urls).await });
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn toggle(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.open.remove(&id) {
            self.open.insert(id);
        }
        self.load_bodies(cx);
        self.fetch_images(cx);
        cx.notify();
    }

    pub fn last_message(&self) -> Option<i64> {
        self.thread.as_ref().and_then(|t| t.messages.last()).map(|m| m.id)
    }

    /// `i`: allow remote images in the open messages.
    pub fn load_images(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self.open.iter().copied().collect();
        self.images_on.extend(ids);
        self.fetch_images(cx);
        cx.notify();
    }

    /// The newest open message's HTML, for "open original".
    pub fn original_html(&self) -> Option<String> {
        let t = self.thread.as_ref()?;
        let m = t.messages.iter().rev().find(|m| self.open.contains(&m.id))?;
        m.html.clone().or_else(|| m.text.as_deref().map(|x| format!("<pre style=\"white-space:pre-wrap\">{}</pre>", text_to_html(x))))
    }

    pub fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        let o = self.scroll.offset();
        let max = self.scroll.max_offset().y;
        let y = (o.y - px(dy)).max(-max).min(px(0.));
        self.scroll.set_offset(point(o.x, y));
        cx.notify();
    }

    pub fn page_height(&self) -> f32 {
        let h = self.scroll.bounds().size.height;
        if h > px(0.) { f32::from(h) } else { 600. }
    }

    fn render_message(&self, m: &Message, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.open.contains(&m.id);
        let id = m.id;
        let who = m.from.as_ref().map(|a| a.display().to_string()).unwrap_or_else(|| "(unknown)".into());
        let head = h_flex()
            .id(("head", id as u64))
            .gap_3()
            .px_4()
            .py_3()
            .items_center()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, cx)))
            .child(
                div()
                    .size(px(32.))
                    .flex_none()
                    .rounded_full()
                    .bg(pal.active)
                    .text_color(pal.strong)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(initials(m)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_color(pal.strong).font_weight(FontWeight::MEDIUM).child(who))
                            .when(open, |d| {
                                d.when_some(m.from.as_ref().filter(|a| a.name.is_some()), |d, a| {
                                    d.child(div().text_color(pal.faint).text_xs().child(a.address.clone()))
                                })
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(pal.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(if open {
                                let mut s = format!("to {}", if m.to.is_empty() { "—".into() } else { addr_list(&m.to) });
                                if !m.cc.is_empty() {
                                    s.push_str(&format!(" · cc {}", addr_list(&m.cc)));
                                }
                                s
                            } else {
                                m.snippet.clone()
                            }),
                    ),
            )
            .child(div().flex_none().text_xs().text_color(pal.faint).child(if open { long_date(m.date) } else { short_date(m.date) }));

        let card = v_flex().border_1().border_color(pal.line).rounded_lg().bg(pal.card).overflow_hidden().child(head);
        if !open {
            return card.into_any_element();
        }

        let images_on = self.images_on.contains(&id);
        let body: AnyElement = if !m.has_body {
            div().px_4().pb_4().text_sm().text_color(pal.faint).child("Loading…").into_any_element()
        } else {
            let html = match (&m.html, &m.text) {
                (Some(h), _) => prepare_html(h),
                (None, Some(t)) => text_to_html(t),
                _ => String::new(),
            };
            let resolver = self.images.resolver(images_on);
            div()
                .px_4()
                .pb_4()
                .pl(px(60.))
                .text_sm()
                .text_color(pal.text)
                .child(
                    TextView::html(("body", id as u64), html)
                        .selectable(true)
                        .image_source(resolver)
                        .on_link_click(|url, _, _, cx| {
                            if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
                                cx.open_url(url);
                            }
                        }),
                )
                .into_any_element()
        };

        let remote = m.html.as_deref().is_some_and(has_remote_images);
        let tools = h_flex()
            .px_4()
            .pb_3()
            .pl(px(56.))
            .gap_1()
            .child(
                Button::new(("reply", id as u64))
                    .ghost()
                    .small()
                    .icon(IconName::Reply)
                    .label("Reply")
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(ThreadEvent::Reply(id, false)))),
            )
            .child(
                Button::new(("reply-all", id as u64))
                    .ghost()
                    .small()
                    .icon(IconName::ReplyAll)
                    .label("Reply all")
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(ThreadEvent::Reply(id, true)))),
            )
            .child(div().flex_1())
            .when(remote && !images_on, |d| {
                d.child(
                    Button::new(("images", id as u64))
                        .ghost()
                        .small()
                        .icon(IconName::Image)
                        .label("Load images")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.images_on.insert(id);
                            this.fetch_images(cx);
                            cx.notify();
                        })),
                )
            })
            .when_some(m.from.clone().filter(|_| remote), |d, from| {
                let sender = from.address.to_lowercase();
                let core = self.core.clone();
                let always = core.get_settings().image_senders.contains(&sender);
                d.child(
                    Button::new(("always", id as u64))
                        .ghost()
                        .small()
                        .label(if always { "Block images from sender" } else { "Always load from sender" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut list = this.core.get_settings().image_senders;
                            if always {
                                list.retain(|s| s != &sender);
                            } else {
                                list.push(sender.clone());
                                this.images_on.insert(id);
                            }
                            let _ = this.core.set_settings(serde_json::json!({ "imageSenders": list }));
                            this.fetch_images(cx);
                            cx.notify();
                        })),
                )
            });

        let attachments = (!m.attachments.is_empty()).then(|| {
            h_flex().px_4().pb_3().pl(px(60.)).gap_2().flex_wrap().children(m.attachments.iter().map(|a| {
                h_flex()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(pal.line)
                    .text_xs()
                    .text_color(pal.muted)
                    .child(Icon::new(IconName::Paperclip).size_3())
                    .child(format!("{} · {} KB", a.filename, a.size.div_ceil(1024)))
            }))
        });

        card.child(body).children(attachments).child(tools).into_any_element()
    }
}

impl Render for ThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = cx.global::<Palette>().clone();
        let Some(t) = self.thread.clone() else { return div().into_any_element() };
        let messages: Vec<AnyElement> = t.messages.iter().map(|m| self.render_message(m, &pal, cx)).collect();
        let hint = |k: &'static [&'static str], label: &'static str| {
            h_flex()
                .gap_1()
                .children(k.iter().map(|k| kbd(k, &pal)))
                .child(div().text_color(pal.faint).child(label))
        };
        div()
            .id("thread-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(880.))
                    .mx_auto()
                    .px_6()
                    .py_5()
                    .gap_3()
                    .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).text_color(pal.strong).pb_2().child(t.subject.clone()))
                    .children(messages)
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_4()
                            .pt_2()
                            .pb_6()
                            .text_xs()
                            .child(hint(&["j", "k"], "scroll"))
                            .child(hint(&["J", "K"], "next / prev"))
                            .child(hint(&["r"], "reply"))
                            .child(hint(&["e"], "archive"))
                            .child(hint(&["i"], "images"))
                            .child(hint(&["q"], "back"))
                            .child(hint(&["?"], "all keys")),
                    ),
            )
            .into_any_element()
    }
}

pub fn kbd(k: &str, pal: &Palette) -> Div {
    div()
        .px(px(5.))
        .min_w(px(18.))
        .h(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(pal.line)
        .bg(pal.hover)
        .text_color(pal.muted)
        .text_xs()
        .child(k.to_string())
}
