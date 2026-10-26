//! Puts NewQ's icon and version details into the Windows executable, so the
//! file, the taskbar and the shortcut show the logo. Nothing happens on other
//! systems, and if the resource compiler can't be found the build goes on
//! without the icon.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/logo/newq.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/logo/newq.ico");
        res.set("ProductName", "NewQ");
        res.set(
            "FileDescription",
            "NewQ: cue-based playback for theatre and live events",
        );
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not put the icon in the Windows executable: {e}");
        }
    }
}
