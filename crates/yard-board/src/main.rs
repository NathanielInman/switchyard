//! `yard-board`: the Switchyard panel. A GTK4 layer-shell window that stays
//! resident while hidden and opens from wherever the bar was clicked.

#![cfg_attr(not(target_os = "linux"), allow(unused))]

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("yard-board is the Linux (Wayland layer-shell) board; on this system use yard-tray");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
mod anim;
#[cfg(target_os = "linux")]
mod board;
#[cfg(target_os = "linux")]
mod historyview;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod logview;
#[cfg(target_os = "linux")]
mod stage;
#[cfg(target_os = "linux")]
mod tile;
#[cfg(target_os = "linux")]
fn main() -> gtk::glib::ExitCode {
    linux::main()
}
