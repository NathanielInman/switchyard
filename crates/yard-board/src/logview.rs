//! The log page: a task's log, opened on its first real error.

use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use yard_core::model::{Phase, TaskView};

const MAX_BYTES: u64 = 768 * 1024;

pub struct LogView {
    pub root: gtk::Box,
    lamp: gtk::Label,
    name: gtk::Label,
    message: gtk::Label,
    path: gtk::Label,
    text: gtk::TextView,
    scroller: gtk::ScrolledWindow,
    pub copy_btn: gtk::Button,
    pub open_btn: gtk::Button,
    pub run_btn: gtk::Button,
    pub follow_btn: gtk::Button,
    pub ack_btn: gtk::Button,
    pub back_btn: gtk::Button,
    pub task: RefCell<Option<String>>,
    file: RefCell<Option<PathBuf>>,
    offset: Cell<u64>,
    pub follow: Cell<bool>,
    lines: RefCell<Vec<String>>,
    error_line: Cell<Option<usize>>,
}

fn button(label: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_focusable(false);
    b
}

impl LogView {
    pub fn new() -> LogView {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("loghead");
        let lamp = gtk::Label::new(Some("●"));
        lamp.add_css_class("lamp");
        let name = gtk::Label::new(None);
        name.add_css_class("name");
        let message = gtk::Label::new(None);
        message.add_css_class("message");
        message.set_ellipsize(gtk::pango::EllipsizeMode::End);
        message.set_hexpand(true);
        message.set_xalign(0.0);
        head.append(&lamp);
        head.append(&name);
        head.append(&message);

        let text = gtk::TextView::new();
        text.set_editable(false);
        text.set_cursor_visible(false);
        text.set_monospace(true);
        text.set_wrap_mode(gtk::WrapMode::WordChar);
        text.add_css_class("log");
        text.set_left_margin(10);
        text.set_right_margin(10);
        text.set_top_margin(8);
        text.set_bottom_margin(8);
        text.set_focusable(false);
        let buf = text.buffer();
        let table = buf.tag_table();
        table.add(&gtk::TextTag::builder().name("err").paragraph_background("#4a2a2f").foreground("#ffd5d9").build());
        table.add(&gtk::TextTag::builder().name("mark").foreground("#88c0d0").build());
        table.add(&gtk::TextTag::builder().name("good").foreground("#a3be8c").build());
        table.add(&gtk::TextTag::builder().name("bad").foreground("#e0848c").build());

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&text));
        scroller.set_min_content_height(300);
        scroller.set_propagate_natural_height(false);
        scroller.set_vexpand(true);
        scroller.add_css_class("logwell");

        let path = gtk::Label::new(None);
        path.add_css_class("dim");
        path.set_xalign(0.0);
        path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.add_css_class("actions");
        let copy_btn = button("y  copy error");
        let open_btn = button("e  open");
        let run_btn = button("r  run again");
        let follow_btn = button("f  follow");
        let ack_btn = button("a  acknowledge");
        let back_btn = button("esc  back");
        for b in [&copy_btn, &open_btn, &run_btn, &follow_btn, &ack_btn, &back_btn] {
            actions.append(b);
        }

        root.append(&head);
        root.append(&scroller);
        root.append(&actions);
        LogView {
            root,
            lamp,
            name,
            message,
            path,
            text,
            scroller,
            copy_btn,
            open_btn,
            run_btn,
            follow_btn,
            ack_btn,
            back_btn,
            task: RefCell::new(None),
            file: RefCell::new(None),
            offset: Cell::new(0),
            follow: Cell::new(false),
            lines: RefCell::new(vec![]),
            error_line: Cell::new(None),
        }
    }

    pub fn update_head(&self, v: &TaskView) {
        for c in crate::tile::PHASE_CLASSES {
            self.lamp.remove_css_class(c);
        }
        self.lamp.add_css_class(v.phase.as_str());
        self.name.set_text(&v.label);
        let now = crate::board::now_corrected();
        let msg = match v.phase {
            Phase::Failed | Phase::Seen => v.message.clone().unwrap_or_else(|| "failed".into()),
            _ => crate::tile::status_line(v, now),
        };
        self.message.set_text(&msg);
        for c in ["message"] {
            if matches!(v.phase, Phase::Failed | Phase::Seen) {
                self.message.add_css_class(c);
            } else {
                self.message.remove_css_class(c);
            }
        }
        self.path.set_text(&v.log);
        let has_err = v.error.is_some();
        self.copy_btn.set_label(if has_err { "y  copy error" } else { "y  copy tail" });
        if has_err {
            self.copy_btn.add_css_class("primary");
        } else {
            self.copy_btn.remove_css_class("primary");
        }
        let loc = v.error.as_ref().and_then(|e| e.line.map(|l| (e.file.clone(), l)));
        match loc {
            Some((_, l)) => self.open_btn.set_label(&format!("e  open :{l}")),
            None => self.open_btn.set_label("e  open repo"),
        }
        self.ack_btn.set_visible(matches!(v.phase, Phase::Failed | Phase::Seen));
        if self.follow.get() {
            self.follow_btn.add_css_class("active");
        } else {
            self.follow_btn.remove_css_class("active");
        }
    }

    /// Loads a task's log and scrolls to its first error.
    pub fn load(&self, v: &TaskView) {
        *self.task.borrow_mut() = Some(v.id.clone());
        let path = PathBuf::from(&v.log);
        *self.file.borrow_mut() = Some(path.clone());
        self.follow.set(matches!(v.phase, Phase::Building | Phase::Running));
        let text = read_tail(&path, 0, MAX_BYTES).map(|(t, end)| {
            self.offset.set(end);
            t
        });
        let text = text.unwrap_or_else(|| {
            self.offset.set(0);
            format!("No log yet for {}.\nIt will appear at {} on the first run.", v.label, v.log)
        });
        let buf = self.text.buffer();
        buf.set_text(&text);
        *self.lines.borrow_mut() = text.lines().map(String::from).collect();
        self.highlight(0);
        self.error_line.set(None);
        if let Some(e) = &v.error {
            let lines = self.lines.borrow();
            let target = e.text.trim_end();
            if let Some(i) = lines.iter().rposition(|l| yard_core::errors::strip_ansi(l).trim_end() == target) {
                self.error_line.set(Some(i));
            }
        }
        if let Some(i) = self.error_line.get() {
            let lines = self.lines.borrow();
            let mut end = i;
            while end + 1 < lines.len() && end < i + 4 {
                let next = &lines[end + 1];
                if next.trim().is_empty() || next.starts_with("=====") || !next.starts_with(char::is_whitespace) {
                    break;
                }
                end += 1;
            }
            drop(lines);
            if let (Some(s), Some(mut e)) = (buf.iter_at_line(i as i32), buf.iter_at_line(end as i32)) {
                e.forward_to_line_end();
                buf.apply_tag_by_name("err", &s, &e);
            }
            self.scroll_to_line(i);
        } else {
            self.scroll_to_end();
        }
        self.update_head(v);
    }

    fn highlight(&self, from_line: usize) {
        let buf = self.text.buffer();
        let lines = self.lines.borrow();
        for (i, l) in lines.iter().enumerate().skip(from_line) {
            let tag = if l.starts_with("===== build ok") || l.starts_with("===== run ended · exit 0") {
                "good"
            } else if l.starts_with("===== exit") || l.starts_with("===== run ended") {
                "bad"
            } else if l.starts_with("=====") {
                "mark"
            } else {
                continue;
            };
            if let (Some(s), Some(mut e)) = (buf.iter_at_line(i as i32), buf.iter_at_line(i as i32)) {
                e.forward_to_line_end();
                buf.apply_tag_by_name(tag, &s, &e);
            }
        }
    }

    fn scroll_to_line(&self, line: usize) {
        let buf = self.text.buffer();
        let Some(iter) = buf.iter_at_line(line.saturating_sub(3) as i32) else { return };
        let mark = buf.create_mark(None, &iter, true);
        let tv = self.text.clone();
        gtk::glib::idle_add_local_once(move || {
            tv.scroll_to_mark(&mark, 0.0, true, 0.0, 0.0);
        });
    }

    fn scroll_to_end(&self) {
        let adj = self.scroller.vadjustment();
        gtk::glib::idle_add_local_once(move || {
            adj.set_value(adj.upper() - adj.page_size());
        });
    }

    /// Appends whatever the log gained since the last read.
    pub fn poll(&self) {
        let Some(path) = self.file.borrow().clone() else { return };
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if len == self.offset.get() {
            return;
        }
        if len < self.offset.get() {
            // A new run rotated the log: start over.
            if let Some((t, end)) = read_tail(&path, 0, MAX_BYTES) {
                self.offset.set(end);
                self.text.buffer().set_text(&t);
                *self.lines.borrow_mut() = t.lines().map(String::from).collect();
                self.highlight(0);
                if self.follow.get() {
                    self.scroll_to_end();
                }
            }
            return;
        }
        let Some((t, end)) = read_tail(&path, self.offset.get(), MAX_BYTES) else { return };
        // Only take whole lines.
        let Some(cut) = t.rfind('\n') else { return };
        let chunk = &t[..=cut];
        self.offset.set(end - (t.len() - cut - 1) as u64);
        let buf = self.text.buffer();
        let mut iter = buf.end_iter();
        buf.insert(&mut iter, chunk);
        let from = self.lines.borrow().len();
        self.lines.borrow_mut().extend(chunk.lines().map(String::from));
        self.highlight(from);
        if self.follow.get() {
            self.scroll_to_end();
        }
    }

    pub fn toggle_follow(&self) {
        self.follow.set(!self.follow.get());
        if self.follow.get() {
            self.follow_btn.add_css_class("active");
            self.scroll_to_end();
        } else {
            self.follow_btn.remove_css_class("active");
        }
    }

    /// The error line plus a few lines of context, or the last lines.
    pub fn error_excerpt(&self) -> String {
        let lines = self.lines.borrow();
        match self.error_line.get() {
            Some(i) => lines[i..(i + 6).min(lines.len())].join("\n"),
            None => {
                let start = lines.len().saturating_sub(20);
                lines[start..].join("\n")
            }
        }
    }

    pub fn scroll_by(&self, pages: f64) {
        let adj = self.scroller.vadjustment();
        adj.set_value((adj.value() + adj.page_size() * pages).clamp(0.0, adj.upper() - adj.page_size()));
    }
}

fn read_tail(path: &PathBuf, from: u64, max: u64) -> Option<(String, u64)> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = if len.saturating_sub(from) > max { len - max } else { from };
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).to_string();
    if start > from && start > 0 {
        // Started mid-line: drop the partial first line.
        if let Some(p) = text.find('\n') {
            text = text[p + 1..].to_string();
        }
    }
    Some((text, start + buf.len() as u64))
}
