//! Embed the Windows executable icon (`packing/windows/dsh.ico`) as a
//! resource.
//!
//! The raster icons are generated from the full-resolution vector sources
//! `assets/dsh-black.svg` / `assets/dsh-white.svg` with ffmpeg (see the
//! README): `packing/windows/dsh.ico` is the black 16/32/48/64/128/256 icon
//! embedded here as resource 101, and `packing/windows/dsh-white.ico` is the
//! white "night" variant loaded from memory at runtime on dark themes.

fn main() {
    // Build scripts compile for the HOST with a special cfg set (test,
    // debug_assertions, the host's `target_family` and `host`) — `target_os`
    // is NOT set there, so `#[cfg(windows)]` sees the host (and would run the
    // winres step while cross-checking macOS/Linux targets), while
    // `#[cfg(target_os = "windows")]` would be false on EVERY host (and the
    // .exe icon resource would silently vanish). The only reliable way to
    // gate on the actual TARGET is the `CARGO_CFG_TARGET_OS` env var.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // User-visible product metadata on the .exe (Explorer properties,
        // taskbar tooltips, Details-tab description, Win10 process list).
        // The binary itself stays named dshl.exe; the names shown to users
        // are the launcher's own — this is a third-party launcher for the
        // DeepSeek Harness, not an official DeepSeek product.
        res.set("ProductName", "DSHL");
        res.set("FileDescription", "DeepSeek Harness Launcher");
        res.set("InternalName", "dshl");
        // winres defaults to a *neutral* language (0), which lenient readers
        // (Explorer, Get-Item) handle but stricter ones (Task Manager's
        // process-list naming) can miss, falling back to the bare exe name.
        // Use the standard en-US language id, like normal VS-built binaries.
        res.set_language(0x0409);
        // webui loads the WebView window icon via
        // `LoadIcon(hInstance, MAKEINTRESOURCE(101))`, so the resource must be
        // embedded with id 101 (matching `add_url.py`'s `101 ICON "..."`).
        res.set_icon_with_id("../../packing/windows/dsh.ico", "101");
        if let Err(e) = res.compile() {
            panic!("failed to embed Windows icon resource: {e}");
        }
    }

    // Re-run the build script if any icon (source or generated) changes.
    println!("cargo:rerun-if-changed=../../assets/dsh-black.svg");
    println!("cargo:rerun-if-changed=.././assets/dsh-white.svg");
    println!("cargo:rerun-if-changed=../../packing/windows/dsh.ico");
    println!("cargo:rerun-if-changed=../../packing/windows/dsh-white.ico");

    // webui's macOS backend (wkwebview.m) uses WKWebView, but webui-rs's
    // build.rs never emits `-framework WebKit` (it links nothing extra on
    // macOS). Without it the link fails with undefined `_OBJC_CLASS_$_WKWebView`.
    // Link flags from any build script in the graph are merged, so we add the
    // framework here. Cocoa too — webui links both (see its CMakeLists).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=WebKit");
        println!("cargo:rustc-link-lib=framework=Cocoa");
    }
}
