//! The conversation list: rows, cursor, selection and the "don't move things under me" merge.

use std::collections::{HashMap, HashSet};

use gpui_kit::{Pixels, ScrollStrategy, UniformListScrollHandle, px};
use yamc_core::types::{ThreadKey, ThreadRow};

pub const ROW: f32 = 38.;
pub const PAGE: usize = 300;

/// Keep rows where they are while the view is open: updated rows keep their slot, new mail goes
/// on top, rows that only just came into the fetch window go at the bottom, gone rows drop out.
pub fn merge_stable(prev: &[ThreadRow], next: Vec<ThreadRow>, local: &HashMap<ThreadKey, bool>) -> Vec<ThreadRow> {
    let had: HashSet<ThreadKey> = prev.iter().map(ThreadRow::key).collect();
    let newest = prev.iter().map(|r| r.date).max().unwrap_or(0);
    let mut by_key: HashMap<ThreadKey, ThreadRow> = HashMap::new();
    let mut top = Vec::new();
    let mut bottom = Vec::new();
    for r in next {
        let k = r.key();
        if had.contains(&k) {
            by_key.insert(k, r);
        } else if r.date > newest {
            top.push(r);
        } else {
            bottom.push(r);
        }
    }
    let kept = prev.iter().filter_map(|r| by_key.remove(&r.key()));
    top.into_iter()
        .chain(kept)
        .chain(bottom)
        .map(|mut r| {
            if let Some(u) = local.get(&r.key()) {
                r.unread = *u;
            }
            r
        })
        .collect()
}

pub enum Screen {
    Top,
    Middle,
    Bottom,
}

#[derive(Default)]
pub struct ThreadList {
    pub rows: Vec<ThreadRow>,
    pub cursor: usize,
    pub marked: HashSet<ThreadKey>,
    pub anchor: Option<usize>,
    /// Read state we changed locally; wins over a refresh that raced ahead of the server.
    pub local: HashMap<ThreadKey, bool>,
    pub loaded: bool,
    pub exhausted: bool,
    pub loading_more: bool,
    /// Rows are from the previous query and stay on screen until the new results replace them,
    /// so typing a search doesn't flash an empty list.
    pub stale: bool,
    pub scroll: UniformListScrollHandle,
}

impl ThreadList {
    pub fn reset(&mut self) {
        let scroll = self.scroll.clone();
        *self = Self { scroll, ..Default::default() };
        self.scroll.0.borrow().base_handle.set_offset(gpui_kit::point(px(0.), px(0.)));
    }

    /// Like `reset`, but the old rows stay visible until `apply` replaces them.
    pub fn reset_keep_rows(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        self.reset();
        self.rows = rows;
        self.stale = true;
        self.loaded = true;
    }

    pub fn apply(&mut self, next: Vec<ThreadRow>, want: usize) {
        self.exhausted = next.len() < want;
        self.rows = if self.rows.is_empty() || self.stale { next } else { merge_stable(&self.rows, next, &self.local) };
        self.stale = false;
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        self.loaded = true;
    }

    pub fn append(&mut self, more: Vec<ThreadRow>) {
        self.exhausted = more.len() < PAGE;
        let have: HashSet<ThreadKey> = self.rows.iter().map(ThreadRow::key).collect();
        self.rows.extend(more.into_iter().filter(|r| !have.contains(&r.key())));
        self.loading_more = false;
    }

    pub fn current(&self) -> Option<&ThreadRow> {
        self.rows.get(self.cursor)
    }

    fn viewport(&self) -> (Pixels, Pixels) {
        let s = self.scroll.0.borrow();
        let h = s.base_handle.bounds().size.height;
        (if h > px(0.) { h } else { px(ROW * 10.) }, -s.base_handle.offset().y)
    }

    fn visible_rows(&self) -> usize {
        ((self.viewport().0 / px(ROW)) as usize).max(1)
    }

    fn first_visible(&self) -> usize {
        (self.viewport().1 / px(ROW)).ceil().max(0.) as usize
    }

    pub fn go(&mut self, i: isize) -> Option<&ThreadRow> {
        let n = self.rows.len();
        if n == 0 {
            return None;
        }
        let i = i.clamp(0, n as isize - 1) as usize;
        self.cursor = i;
        if i == 0 {
            self.scroll.0.borrow().base_handle.set_offset(gpui_kit::point(px(0.), px(0.)));
        } else {
            self.scroll.scroll_to_item(i, ScrollStrategy::Nearest);
        }
        self.rows.get(i)
    }

    pub fn move_by(&mut self, delta: isize) -> Option<&ThreadRow> {
        self.go(self.cursor as isize + delta)
    }

    /// Jump to a row; negative counts from the end (-1 = last).
    pub fn move_to(&mut self, i: isize) -> Option<&ThreadRow> {
        let i = if i < 0 { self.rows.len() as isize + i } else { i };
        self.go(i)
    }

    /// Scroll by a fraction of the visible rows and move the cursor with it (<C-d>, <C-f>).
    pub fn page(&mut self, fraction: f32) {
        let mut d = (self.visible_rows() as f32 * fraction).trunc() as isize;
        if d == 0 {
            d = fraction.signum() as isize;
        }
        {
            let s = self.scroll.0.borrow();
            let h = &s.base_handle;
            let max = h.max_offset().y;
            let y = (h.offset().y - px(d as f32 * ROW)).max(-max).min(px(0.));
            h.set_offset(gpui_kit::point(h.offset().x, y));
        }
        let n = self.rows.len() as isize;
        if n > 0 {
            self.cursor = (self.cursor as isize + d).clamp(0, n - 1) as usize;
        }
    }

    /// H / M / L.
    pub fn screen(&mut self, where_: Screen) {
        let top = self.first_visible();
        let last = (top + self.visible_rows()).saturating_sub(1).min(self.rows.len().saturating_sub(1));
        let i = match where_ {
            Screen::Top => top,
            Screen::Bottom => last,
            Screen::Middle => (top + last) / 2,
        };
        self.go(i as isize);
    }

    /// zt / zz / zb.
    pub fn align(&mut self, s: ScrollStrategy) {
        self.scroll.scroll_to_item_strict(self.cursor, s);
    }

    pub fn toggle_select(&mut self) {
        if let Some(k) = self.current().map(ThreadRow::key)
            && !self.marked.remove(&k)
        {
            self.marked.insert(k);
        }
    }

    /// V: start/stop visual mode anchored at the cursor. Returns whether it's on now.
    pub fn visual(&mut self) -> bool {
        self.anchor = if self.anchor.is_some() { None } else { Some(self.cursor) };
        self.anchor.is_some()
    }

    pub fn in_visual(&self, i: usize) -> bool {
        self.anchor.is_some_and(|a| i >= a.min(self.cursor) && i <= a.max(self.cursor))
    }

    pub fn is_selected(&self, i: usize) -> bool {
        self.in_visual(i) || self.rows.get(i).is_some_and(|r| self.marked.contains(&r.key()))
    }

    /// Selected rows (visual range + x-marked), or the cursor row if nothing is selected.
    pub fn targets(&self) -> Vec<ThreadRow> {
        let sel: Vec<ThreadRow> = (0..self.rows.len()).filter(|i| self.is_selected(*i)).map(|i| self.rows[i].clone()).collect();
        if sel.is_empty() { self.current().cloned().into_iter().collect() } else { sel }
    }

    pub fn clear_selection(&mut self) -> bool {
        let had = !self.marked.is_empty() || self.anchor.is_some();
        self.marked.clear();
        self.anchor = None;
        had
    }

    pub fn mark_local(&mut self, key: &ThreadKey, unread: bool) {
        self.local.insert(key.clone(), unread);
        for r in self.rows.iter_mut().filter(|r| &r.key() == key) {
            r.unread = unread;
        }
    }

    pub fn remove_local(&mut self, key: &ThreadKey) {
        if let Some(i) = self.rows.iter().position(|r| &r.key() == key) {
            self.rows.remove(i);
            if i < self.cursor {
                self.cursor -= 1;
            }
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, date: i64, unread: bool) -> ThreadRow {
        ThreadRow {
            account_id: 1,
            thread_id: id.into(),
            subject: id.into(),
            from: String::new(),
            snippet: String::new(),
            date,
            unread,
            count: 1,
        }
    }

    #[test]
    fn stable_merge() {
        let prev = vec![row("a", 30, true), row("b", 20, false), row("c", 10, true)];
        // `a` got read on the server (sorting would move it), `c` is gone, `d` is new, `e` came into the window.
        let next = vec![row("d", 40, true), row("b", 20, false), row("a", 30, false), row("e", 5, false)];
        let merged = merge_stable(&prev, next, &HashMap::new());
        let ids: Vec<_> = merged.iter().map(|r| r.thread_id.as_str()).collect();
        assert_eq!(ids, ["d", "a", "b", "e"]);
        assert!(!merged[1].unread);
        // A local change wins over a stale refresh.
        let local = HashMap::from([(ThreadKey(1, "b".into()), true)]);
        let again = merge_stable(&merged, merged.clone(), &local);
        assert!(again[2].unread);
    }

    #[test]
    fn selection() {
        let mut l = ThreadList { rows: (0..5).map(|i| row(&i.to_string(), 50 - i, true)).collect(), ..Default::default() };
        assert_eq!(l.targets().len(), 1);
        l.visual();
        l.cursor = 2;
        assert_eq!(l.targets().len(), 3);
        l.cursor = 4;
        l.toggle_select();
        l.clear_selection();
        assert_eq!(l.targets().len(), 1);
        l.remove_local(&ThreadKey(1, "4".into()));
        assert_eq!(l.cursor, 3);
    }
}
