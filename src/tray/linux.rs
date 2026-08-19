//! Linux tray support via StatusNotifier (libayatana-appindicator3 + GTK3),
//! loaded at runtime with `dlopen` so the binary never hard-depends on the
//! desktop libs — on systems without the library, `close-to-tray` degrades
//! to the original close-to-exit behaviour with a log line.
//!
//! Unlike Windows (where the close handler intercepts and hides the window),
//! WebKitGTK windows cannot be intercepted, so closing the window leaves the
//! launcher running with dsh alive; the tray icon restores the window
//! (re-created and re-navigated to the dsh URL) or quits.

use std::os::raw::{c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static RESTORE_REQUESTED: AtomicBool = AtomicBool::new(false);
static OPEN_URL_REQUESTED: AtomicBool = AtomicBool::new(false);
/// The dlopen'd library handles, stored as `usize` (raw pointers are neither
/// `Send` nor `Sync`, so they cannot live in a `static`).
static LIBS: OnceLock<(usize, usize)> = OnceLock::new();

type DlsymFn = unsafe extern "C" fn();

unsafe fn sym(handle: *mut c_void, name: &str) -> Option<DlsymFn> {
    let cname = std::ffi::CString::new(name).ok()?;
    // SAFETY: dlsym on a handle we just dlopen'ed; null results are handled.
    let p = unsafe { libc::dlsym(handle, cname.as_ptr()) };
    if p.is_null() {
        None
    } else {
        // SAFETY: the caller only uses symbols that are known to exist in
        // the loaded library (the ones checked right after this).
        Some(unsafe { std::mem::transmute::<*mut c_void, DlsymFn>(p) })
    }
}

/// `dlopen` result as an `Option` (null handle = library not available).
fn dlopen(name: &std::ffi::CStr) -> Option<*mut c_void> {
    // SAFETY: dlopen with a fixed soname; failure returns null and degrades
    // to "no tray".
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() { None } else { Some(handle) }
}

/// The menu callbacks run on the GTK main-loop thread and only flip
/// atomics; the launcher thread acts on them (same pattern as Windows).
unsafe extern "C" fn on_restore(_w: *mut c_void, _d: *mut c_void) {
    RESTORE_REQUESTED.store(true, Ordering::SeqCst);
}
unsafe extern "C" fn on_quit(_w: *mut c_void, _d: *mut c_void) {
    QUIT_REQUESTED.store(true, Ordering::SeqCst);
}
unsafe extern "C" fn on_open_url(_w: *mut c_void, _d: *mut c_void) {
    OPEN_URL_REQUESTED.store(true, Ordering::SeqCst);
}

/// Spawn the AppIndicator thread (idempotent).
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        unsafe {
            // SAFETY: dlopen/dlsym with fixed sonames; failures are logged
            // and degrade gracefully (no tray).
            let appind = dlopen(c"libayatana-appindicator3.so.1");
            let gtk = dlopen(c"libgtk-3.so.0");
            LIBS.get_or_init(|| {
                (
                    appind.map(|h| h as usize).unwrap_or(0),
                    gtk.map(|h| h as usize).unwrap_or(0),
                )
            });
            let (Some(appind), Some(gtk)) = (appind, gtk) else {
                crate::debug::emit(
                    "tray: libayatana-appindicator3 / libgtk-3 not available; close-to-tray disabled",
                );
                STARTED.store(false, Ordering::SeqCst);
                return;
            };

            // GTK3 function pointers. Any missing symbol degrades the
            // tray (log + no-op) instead of panicking on a null fn ptr.
            let (
                Some(gtk_init),
                Some(gtk_main),
                Some(gtk_menu_new),
                Some(gtk_menu_item_new_with_label),
                Some(gtk_menu_shell_append),
                Some(gtk_widget_show_all),
                Some(g_signal_connect),
                Some(indicator_new),
                Some(indicator_set_status),
                Some(indicator_set_menu),
            ) = (
                sym(gtk, "gtk_init"),
                sym(gtk, "gtk_main"),
                sym(gtk, "gtk_menu_new"),
                sym(gtk, "gtk_menu_item_new_with_label"),
                sym(gtk, "gtk_menu_shell_append"),
                sym(gtk, "gtk_widget_show_all"),
                sym(gtk, "g_signal_connect"),
                sym(appind, "app_indicator_new"),
                sym(appind, "app_indicator_set_status"),
                sym(appind, "app_indicator_set_menu"),
            )
            else {
                crate::debug::emit("tray: required symbol missing; close-to-tray disabled");
                STARTED.store(false, Ordering::SeqCst);
                return;
            };
            let gtk_init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char) =
                std::mem::transmute(gtk_init);
            let gtk_main: unsafe extern "C" fn() = gtk_main;
            let gtk_menu_new: unsafe extern "C" fn() -> *mut c_void =
                std::mem::transmute(gtk_menu_new);
            let gtk_menu_item_new_with_label: unsafe extern "C" fn(*const c_char) -> *mut c_void =
                std::mem::transmute(gtk_menu_item_new_with_label);
            let gtk_menu_shell_append: unsafe extern "C" fn(*mut c_void, *mut c_void) =
                std::mem::transmute(gtk_menu_shell_append);
            let gtk_widget_show_all: unsafe extern "C" fn(*mut c_void) =
                std::mem::transmute(gtk_widget_show_all);
            let g_signal_connect: unsafe extern "C" fn(
                *mut c_void,
                *const c_char,
                unsafe extern "C" fn(*mut c_void, *mut c_void),
                *mut c_void,
            ) -> u64 = std::mem::transmute(g_signal_connect);
            let indicator_new: unsafe extern "C" fn(
                *const c_char,
                *const c_char,
                c_int,
            ) -> *mut c_void = std::mem::transmute(indicator_new);
            let indicator_set_status: unsafe extern "C" fn(*mut c_void, c_int) =
                std::mem::transmute(indicator_set_status);
            let indicator_set_menu: unsafe extern "C" fn(*mut c_void, *mut c_void) =
                std::mem::transmute(indicator_set_menu);

            gtk_init(std::ptr::null_mut(), std::ptr::null_mut());
            let id = c"dshl".as_ptr();
            let icon = c"dsh".as_ptr();
            let indicator = indicator_new(
                id, icon, 0, /* APP_INDICATOR_CATEGORY_APPLICATION_STATUS */
            );
            if indicator.is_null() {
                crate::debug::emit("tray: app_indicator_new failed");
                STARTED.store(false, Ordering::SeqCst);
                return;
            }

            let menu = gtk_menu_new();
            let restore = std::ffi::CString::new(t!("tray.restore").as_bytes()).unwrap();
            let open_dsh = std::ffi::CString::new(t!("tray.open_dsh").as_bytes()).unwrap();
            let quit = std::ffi::CString::new(t!("tray.quit").as_bytes()).unwrap();
            let restore_item = gtk_menu_item_new_with_label(restore.as_ptr());
            let open_dsh_item = gtk_menu_item_new_with_label(open_dsh.as_ptr());
            let quit_item = gtk_menu_item_new_with_label(quit.as_ptr());
            g_signal_connect(
                restore_item,
                c"activate".as_ptr(),
                on_restore,
                std::ptr::null_mut(),
            );
            g_signal_connect(
                open_dsh_item,
                c"activate".as_ptr(),
                on_open_url,
                std::ptr::null_mut(),
            );
            g_signal_connect(
                quit_item,
                c"activate".as_ptr(),
                on_quit,
                std::ptr::null_mut(),
            );
            gtk_menu_shell_append(menu, restore_item);
            gtk_menu_shell_append(menu, open_dsh_item);
            gtk_menu_shell_append(menu, quit_item);
            gtk_widget_show_all(menu);
            indicator_set_menu(indicator, menu);
            indicator_set_status(indicator, 1 /* APP_INDICATOR_STATUS_ACTIVE */);
            crate::debug::emit("tray: active (close-to-tray)");
            gtk_main();
        }
    });
}

/// Called when the user closes the window; on Linux the window is already
/// gone, so this only makes sure the tray exists (dsh keeps running).
pub fn hide_to_tray() {
    crate::debug::emit("tray: window closed, keeping dsh running in tray");
}

pub fn quit_requested() -> bool {
    QUIT_REQUESTED.load(Ordering::SeqCst)
}

/// True when the user chose "restore window" from the tray menu.
pub fn restore_requested() -> bool {
    RESTORE_REQUESTED.swap(false, Ordering::SeqCst)
}

/// True when the user chose "打开 dsh" from the tray menu.
pub fn open_url_requested() -> bool {
    OPEN_URL_REQUESTED.swap(false, Ordering::SeqCst)
}

/// The appindicator icon comes from the desktop theme (id "dsh"); nothing to
/// swap on theme change.
pub fn set_icon(_dark: bool) {}

/// Stop the tray thread. The GTK main loop is left running; the process
/// exits right after, so no cleanup is strictly required (dlclose would
/// race the GTK thread).
pub fn shutdown() {}
