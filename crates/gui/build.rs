//! Build script: embed the Windows application icon (and a little version
//! metadata) into the `.exe` as a resource.
//!
//! Because the icon is compiled *into* the binary, the app stays a single
//! self-contained executable — there is nothing to ship alongside it. Windows
//! uses this embedded icon everywhere the program shows up: Explorer, the
//! taskbar, and the GPUI window title bar.

fn main() {
    // `CARGO_CFG_WINDOWS` is set by Cargo when the *target* is Windows, so this
    // block is skipped (and the build still succeeds) on any other target.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Only re-run the resource compiler when the icon actually changes.
        println!("cargo:rerun-if-changed=assets/icon.ico");

        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Replayless");
        res.set("FileDescription", "Replayless — GPU video compressor");
        res.compile()
            .expect("failed to embed Windows icon resource (is the Windows SDK rc.exe available?)");
    }
}
