//! Embed the Windows executable icon (`packing/windows/dsh.ico`) as a
//! resource.
//!
//! The raster icons are generated from the full-resolution vector sources
//! `assets/dsh-black.svg` / `assets/dsh-white.svg` with ffmpeg (see the
//! README): `packing/windows/dsh.ico` is the black 16/32/48/64/128/256 icon
//! embedded here as resource 101, and `packing/windows/dsh-white.ico` is the
//! white "night" variant loaded from memory at runtime on dark themes.

fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        // User-visible product metadata on the .exe (Explorer properties,
        // taskbar tooltips, Details-tab description, Win10 process list).
        // The binary itself stays named dshl.exe; the names shown to users
        // are the launcher's own — this is a third-party launcher for the
        // DeepSeek Harness, not an official DeepSeek product.
        let version = env!("CARGO_PKG_VERSION");
        res.set("ProductName", "DSHL");
        res.set("FileDescription", "DeepSeek Harness Launcher");
        res.set("OriginalFilename", "dshl.exe");
        res.set("InternalName", "dshl");
        res.set("ProductVersion", version);
        res.set("FileVersion", version);
        // winres defaults to a *neutral* language (0), which lenient readers
        // (Explorer, Get-Item) handle but stricter ones (Task Manager's
        // process-list naming) can miss, falling back to the bare exe name.
        // Use the standard en-US language id, like normal VS-built binaries.
        res.set_language(0x0409);
        // webui loads the WebView window icon via
        // `LoadIcon(hInstance, MAKEINTRESOURCE(101))`, so the resource must be
        // embedded with id 101 (matching `add_url.py`'s `101 ICON "..."`).
        res.set_icon_with_id("packing/windows/dsh.ico", "101");
        if let Err(e) = res.compile() {
            panic!("failed to embed Windows icon resource: {e}");
        }
    }

    // Re-run the build script if any icon (source or generated) changes.
    println!("cargo:rerun-if-changed=assets/dsh-black.svg");
    println!("cargo:rerun-if-changed=assets/dsh-white.svg");
    println!("cargo:rerun-if-changed=packing/windows/dsh.ico");
    println!("cargo:rerun-if-changed=packing/windows/dsh-white.ico");
}
