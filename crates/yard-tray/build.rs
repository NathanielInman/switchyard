//! Embeds the Windows icon and version info (assets/yard-tray.rc), so the
//! taskbar, Start and Task Manager say "Switchyard" with the signal icon
//! rather than "yard-tray.exe" with a blank one. Nothing on other targets.

fn main() {
    println!("cargo:rerun-if-changed=assets/yard-tray.rc");
    println!("cargo:rerun-if-changed=assets/switchyard.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let v = |k: &str| std::env::var(k).expect("cargo sets the package version");
    let macros = [
        format!("YARD_MAJOR={}", v("CARGO_PKG_VERSION_MAJOR")),
        format!("YARD_MINOR={}", v("CARGO_PKG_VERSION_MINOR")),
        format!("YARD_PATCH={}", v("CARGO_PKG_VERSION_PATCH")),
        format!("YARD_VERSION=\"{}\"", v("CARGO_PKG_VERSION")),
    ];
    embed_resource::compile("assets/yard-tray.rc", &macros).manifest_required().expect("compile the Windows resources");
}
