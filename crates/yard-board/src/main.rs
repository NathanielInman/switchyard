//! `yard-board`: the Switchyard panel. A GTK4 layer-shell window that stays
//! resident while hidden and opens from wherever the bar was clicked.

mod anim;
mod board;
mod logview;
mod stage;
mod tile;

use board::{Board, Msg};
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::time::Duration;
use yard_core::protocol::SubKind;

const DEFAULT_CSS: &str = include_str!("style.css");

thread_local! {
    static FONT_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

fn load_css() {
    let Some(display) = gdk::Display::default() else { return };
    let base = gtk::CssProvider::new();
    base.load_from_string(DEFAULT_CSS);
    gtk::style_context_add_provider_for_display(&display, &base, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    let user_file = yard_core::paths::style_file();
    if user_file.exists() {
        let user = gtk::CssProvider::new();
        user.load_from_path(&user_file);
        gtk::style_context_add_provider_for_display(&display, &user, gtk::STYLE_PROVIDER_PRIORITY_USER);
    }
}

/// Applies `board.font` from the config.
pub fn apply_font(font: &str) {
    let Some(display) = gdk::Display::default() else { return };
    FONT_PROVIDER.with(|slot| {
        let p = slot.borrow_mut().take().unwrap_or_else(|| {
            let p = gtk::CssProvider::new();
            gtk::style_context_add_provider_for_display(&display, &p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
            p
        });
        let family = font.replace('"', "");
        p.load_from_string(&format!("window.switchyard {{ font-family: \"{family}\", monospace; }}"));
        *slot.borrow_mut() = Some(p);
    });
}

/// Keeps a subscription to the daemon open, reconnecting forever.
fn connection(tx: async_channel::Sender<Msg>) {
    loop {
        if let Ok(events) = yard_core::protocol::subscribe(SubKind::Board) {
            let _ = tx.send_blocking(Msg::Connected);
            for ev in events {
                match ev {
                    Ok(e) => {
                        if tx.send_blocking(Msg::Event(e)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send_blocking(Msg::Disconnected);
        }
        std::thread::sleep(Duration::from_millis(800));
    }
}

fn main() -> glib::ExitCode {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("yard-board {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }
    let app = gtk::Application::builder()
        .application_id("io.github.nathanielinman.Switchyard")
        // The daemon decides when a board starts; a second one (a demo, a
        // nested session) must not hand itself to the first over D-Bus.
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let built = std::rc::Rc::new(std::cell::Cell::new(false));
    app.connect_activate(move |app| {
        if built.replace(true) {
            return;
        }
        if !gtk4_layer_shell::is_supported() {
            eprintln!("yard-board: this compositor does not support wlr-layer-shell");
        }
        load_css();
        let (tx, rx) = async_channel::unbounded::<Msg>();
        let board = Board::new(app, tx.clone());
        std::mem::forget(app.hold());
        std::thread::spawn(move || connection(tx));
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                board.handle(msg);
            }
        });
    });
    app.run_with_args::<&str>(&[])
}
