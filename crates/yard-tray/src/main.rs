//! `yard-tray`: Switchyard for macOS and Windows. One resident process with a
//! tray (menu bar) icon that shows the aggregate state, and a hidden board
//! window that opens next to the icon when you click it.
//!
//! On Linux it builds without the tray, for development: `yard-tray --show`
//! opens the board straight away.

mod app;
mod draw;
#[cfg(any(windows, target_os = "macos"))]
mod tray;

use std::process::Command;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("yard-tray {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let show_now = args.iter().any(|a| a == "--show") || cfg!(not(any(windows, target_os = "macos")));
    ensure_daemon();

    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Switchyard")
        .with_inner_size([app::WIDTH, 200.0])
        .with_decorations(false)
        .with_transparent(cfg!(target_os = "macos"))
        .with_resizable(true)
        .with_always_on_top()
        .with_taskbar(false)
        .with_visible(false);
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(target_os = "macos")]
    {
        // A menu bar app: no Dock icon, no app menu.
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            b.with_activation_policy(ActivationPolicy::Accessory);
        }));
    }
    eframe::run_native("Switchyard", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, show_now)))))
}

/// Finds a sibling binary (yard next to yard-tray), then PATH.
pub fn sibling(name: &str) -> std::path::PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let p = dir.join(&file);
        if p.exists() {
            return p;
        }
    }
    file.into()
}

/// Starts the daemon when nothing answers.
fn ensure_daemon() {
    if yard_core::transport::daemon_alive() {
        return;
    }
    let mut c = Command::new(sibling("yard"));
    c.arg("daemon");
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    detach(&mut c);
    if let Ok(mut child) = c.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    for _ in 0..40 {
        if yard_core::transport::daemon_alive() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

pub fn detach(c: &mut Command) {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            libc_setsid();
            Ok(())
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
}

#[cfg(unix)]
fn libc_setsid() {
    extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        setsid();
    }
}
