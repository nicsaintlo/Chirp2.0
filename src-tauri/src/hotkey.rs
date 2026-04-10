/// Global hotkey management.
///
/// Windows: uses tauri-plugin-global-shortcut (RegisterHotKey Win32 API) for
/// press detection, and GetAsyncKeyState polling for release detection.
/// No low-level keyboard hook (WH_KEYBOARD_LL) is installed — this avoids the
/// AV/EDR heuristic that flags keylogger-style code.
///
/// macOS: uses native_hotkey (Carbon CGEventTap), unchanged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "macos")]
use crate::native_hotkey::{self, KeyAction};

// ---------------------------------------------------------------------------
// CapturedKey — returned by capture_next_key()
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct CapturedKey {
    pub code: String,
    pub label: String,
}

// ---------------------------------------------------------------------------
// Label mapping: identifier -> human-readable display label (shared)
// ---------------------------------------------------------------------------

pub fn id_to_label(id: &str) -> String {
    match id {
        "ControlLeft" | "ControlRight" => "Ctrl".into(),
        "ShiftLeft" | "ShiftRight" => "Shift".into(),
        "Alt" | "AltLeft" => {
            if cfg!(target_os = "macos") { "Option".into() } else { "Alt".into() }
        }
        "AltGr" | "AltRight" => "AltGr".into(),
        "MetaLeft" | "MetaRight" => {
            if cfg!(target_os = "macos") { "Cmd".into() } else { "Win".into() }
        }
        "Fn" => "fn".into(),
        s if s.starts_with("Key") && s.len() == 4 => s[3..].into(),
        s if s.starts_with("Digit") && s.len() == 6 => s[5..].into(),
        "ArrowUp" => "Up".into(),
        "ArrowDown" => "Down".into(),
        "ArrowLeft" => "Left".into(),
        "ArrowRight" => "Right".into(),
        "Backquote" => "`".into(),
        "Minus" => "-".into(),
        "Equal" => "=".into(),
        "BracketLeft" => "[".into(),
        "BracketRight" => "]".into(),
        "Semicolon" => ";".into(),
        "Quote" => "'".into(),
        "Backslash" | "IntlBackslash" => "\\".into(),
        "Comma" => ",".into(),
        "Period" => ".".into(),
        "Slash" => "/".into(),
        other => other.into(),
    }
}

// ---------------------------------------------------------------------------
// Parse a hotkey string like "ControlLeft+ShiftLeft+KeyA" — macOS only
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn parse_hotkey(hotkey: &str) -> std::collections::HashSet<String> {
    hotkey.split('+').map(|s| s.trim().to_string()).collect()
}

#[cfg(target_os = "macos")]
const MODIFIERS: &[&str] = &[
    "ControlLeft", "ControlRight",
    "ShiftLeft", "ShiftRight",
    "Alt", "AltLeft", "AltGr", "AltRight",
    "MetaLeft", "MetaRight",
    "Fn",
];

#[cfg(target_os = "macos")]
fn is_modifier_only_combo(combo: &std::collections::HashSet<String>) -> bool {
    combo.iter().all(|k| MODIFIERS.contains(&k.as_str()))
}

// ===========================================================================
// macOS: native_hotkey (CGEventTap) — unchanged
// ===========================================================================

#[cfg(target_os = "macos")]
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static GRAB_THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

#[cfg(target_os = "macos")]
pub fn start(hotkey: &str, app_handle: AppHandle) -> Result<(), String> {
    stop();
    SHUTDOWN.store(false, Ordering::SeqCst);

    let combo = parse_hotkey(hotkey);
    let modifier_only = is_modifier_only_combo(&combo);
    let app = app_handle.clone();

    let handle = std::thread::Builder::new()
        .name("hotkey-grab".into())
        .spawn(move || {
            use std::cell::RefCell;
            let held: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());
            let combo_active: RefCell<bool> = RefCell::new(false);

            let combo_for_cb = combo.clone();
            let app_for_cb = app.clone();

            let grab_result = native_hotkey::grab(move |action: KeyAction| -> bool {
                if SHUTDOWN.load(Ordering::SeqCst) {
                    native_hotkey::stop_run_loop();
                    return true;
                }

                match action {
                    KeyAction::Press(id) => {
                        held.borrow_mut().insert(id.clone());

                        if *held.borrow() == combo_for_cb {
                            if !*combo_active.borrow() {
                                *combo_active.borrow_mut() = true;
                                log::info!("Hotkey pressed");
                                let _ = app_for_cb.emit("hotkey-pressed", ());
                            }
                            if !modifier_only { return false; }
                        }

                        if *combo_active.borrow() && combo_for_cb.contains(&id) && !modifier_only {
                            return false;
                        }
                        true
                    }
                    KeyAction::Release(id) => {
                        held.borrow_mut().remove(&id);

                        if *combo_active.borrow() && combo_for_cb.contains(&id) {
                            *combo_active.borrow_mut() = false;
                            log::info!("Hotkey released");
                            let _ = app_for_cb.emit("hotkey-released", ());
                            if !modifier_only { return false; }
                        }
                        true
                    }
                }
            });

            if let Err(e) = grab_result {
                log::warn!("native grab failed ({e}), falling back to listen-only");
                let _ = app.emit("hotkey-status", "accessibility_required");

                let combo_for_listen = combo;
                let app_for_listen = app;
                let mut held: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut combo_active = false;

                let _ = native_hotkey::listen(move |action: KeyAction| {
                    if SHUTDOWN.load(Ordering::SeqCst) {
                        native_hotkey::stop_run_loop();
                        return;
                    }
                    match action {
                        KeyAction::Press(id) => {
                            held.insert(id);
                            if held == combo_for_listen && !combo_active {
                                combo_active = true;
                                log::info!("Hotkey pressed (listen mode)");
                                let _ = app_for_listen.emit("hotkey-pressed", ());
                            }
                        }
                        KeyAction::Release(id) => {
                            held.remove(&id);
                            if combo_active && combo_for_listen.contains(&id) {
                                combo_active = false;
                                log::info!("Hotkey released (listen mode)");
                                let _ = app_for_listen.emit("hotkey-released", ());
                            }
                        }
                    }
                });
            }
        })
        .map_err(|e| format!("Failed to spawn hotkey thread: {e}"))?;

    if let Ok(mut guard) = GRAB_THREAD.lock() {
        *guard = Some(handle);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn stop() {
    SHUTDOWN.store(true, Ordering::SeqCst);
    native_hotkey::stop_run_loop();
    if let Ok(mut guard) = GRAB_THREAD.lock() {
        *guard = None;
    }
}

#[cfg(target_os = "macos")]
pub fn update(hotkey: &str, app_handle: AppHandle) -> Result<(), String> {
    stop();
    std::thread::sleep(std::time::Duration::from_millis(100));
    start(hotkey, app_handle)
}

#[cfg(target_os = "macos")]
pub async fn capture_next_key() -> Result<CapturedKey, String> {
    SHUTDOWN.store(true, Ordering::SeqCst);
    native_hotkey::stop_run_loop();

    let (tx, rx) = tokio::sync::oneshot::channel::<CapturedKey>();

    std::thread::Builder::new()
        .name("hotkey-capture".into())
        .spawn(move || {
            let tx = std::sync::Mutex::new(Some(tx));
            let _ = native_hotkey::listen(move |action: KeyAction| {
                if let KeyAction::Press(id) = action {
                    let label = crate::hotkey::id_to_label(&id);
                    if let Ok(mut guard) = tx.lock() {
                        if let Some(sender) = guard.take() {
                            let _ = sender.send(CapturedKey { code: id, label });
                            native_hotkey::stop_run_loop();
                        }
                    }
                }
            });
        })
        .map_err(|e| format!("Failed to spawn capture thread: {e}"))?;

    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
        Ok(Ok(key)) => Ok(key),
        Ok(Err(_)) => Err("Capture channel closed unexpectedly".into()),
        Err(_) => Err("Hotkey capture timed out after 10 seconds".into()),
    }
}

// ===========================================================================
// Windows (non-macOS): tauri-plugin-global-shortcut + GetAsyncKeyState polling
//
// RegisterHotKey (used by tauri-plugin-global-shortcut) is the standard OS API
// for global shortcuts used by thousands of legitimate apps. It does NOT install
// a low-level keyboard hook — it only fires when the exact registered combo is
// pressed, and only notifies this app.
//
// Since RegisterHotKey only fires on key-down (not key-up), we use
// GetAsyncKeyState polling in a tiny background thread to detect release.
// GetAsyncKeyState reads the current state of a specific key — it does not
// intercept or capture any keystrokes.
// ===========================================================================

#[cfg(not(target_os = "macos"))]
static CURRENT_SHORTCUT: Mutex<Option<String>> = Mutex::new(None);

#[cfg(not(target_os = "macos"))]
static RELEASE_POLLING: AtomicBool = AtomicBool::new(false);

/// GetAsyncKeyState — reads current key state without hooking.
/// Returns negative (high bit set) if the key is currently held down.
#[cfg(not(target_os = "macos"))]
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vKey: i32) -> i16;
}

/// Convert our internal hotkey format (e.g. "ControlLeft+ShiftLeft+KeyA")
/// to the format expected by tauri-plugin-global-shortcut (e.g. "ctrl+shift+a").
#[cfg(not(target_os = "macos"))]
fn to_shortcut_str(hotkey: &str) -> Result<String, String> {
    let mut mods: Vec<&'static str> = Vec::new();
    let mut key_part: Option<String> = None;

    for part in hotkey.split('+') {
        match part.trim() {
            "ControlLeft" | "ControlRight" => mods.push("ctrl"),
            "ShiftLeft" | "ShiftRight" => mods.push("shift"),
            "Alt" | "AltLeft" => mods.push("alt"),
            "AltGr" | "AltRight" => mods.push("altgr"),
            "MetaLeft" | "MetaRight" => mods.push("super"),
            s => {
                let k = if s.starts_with("Key") && s.len() == 4 {
                    s[3..].to_lowercase()
                } else if s.starts_with("Digit") && s.len() == 6 {
                    s[5..].to_string()
                } else {
                    match s {
                        "Space" => "space".to_string(),
                        "Enter" => "return".to_string(),
                        "Tab" => "tab".to_string(),
                        "Escape" => "escape".to_string(),
                        "Backspace" => "backspace".to_string(),
                        "Delete" => "delete".to_string(),
                        "Insert" => "insert".to_string(),
                        "Home" => "home".to_string(),
                        "End" => "end".to_string(),
                        "PageUp" => "pageup".to_string(),
                        "PageDown" => "pagedown".to_string(),
                        "ArrowUp" => "up".to_string(),
                        "ArrowDown" => "down".to_string(),
                        "ArrowLeft" => "left".to_string(),
                        "ArrowRight" => "right".to_string(),
                        "CapsLock" => "capslock".to_string(),
                        "Backquote" => "grave".to_string(),
                        "Minus" => "minus".to_string(),
                        "Equal" => "equal".to_string(),
                        "BracketLeft" => "leftbracket".to_string(),
                        "BracketRight" => "rightbracket".to_string(),
                        "Semicolon" => "semicolon".to_string(),
                        "Quote" => "apostrophe".to_string(),
                        "Backslash" => "backslash".to_string(),
                        "Comma" => "comma".to_string(),
                        "Period" => "period".to_string(),
                        "Slash" => "slash".to_string(),
                        other if other.starts_with('F') && other.len() <= 3 => other.to_string(),
                        other => return Err(format!("Unsupported key for global shortcut: {other}")),
                    }
                };
                key_part = Some(k);
            }
        }
    }

    let key = key_part.ok_or_else(|| "Hotkey must include at least one non-modifier key".to_string())?;
    let mut result = mods.join("+");
    if !result.is_empty() {
        result.push('+');
    }
    result.push_str(&key);
    Ok(result)
}

/// Map the non-modifier key in a hotkey string to its Windows Virtual Key code.
/// Used to poll GetAsyncKeyState for release detection.
#[cfg(not(target_os = "macos"))]
fn hotkey_to_vk(hotkey: &str) -> Option<i32> {
    for part in hotkey.split('+') {
        let vk = match part.trim() {
            // Skip modifier keys
            "ControlLeft" | "ControlRight" | "ShiftLeft" | "ShiftRight"
            | "Alt" | "AltLeft" | "AltGr" | "AltRight"
            | "MetaLeft" | "MetaRight" => continue,

            s if s.starts_with("Key") && s.len() == 4 => {
                s.chars().nth(3)? as i32 // 'A'=0x41 .. 'Z'=0x5A
            }
            s if s.starts_with("Digit") && s.len() == 6 => {
                s.chars().nth(5)? as i32 // '0'=0x30 .. '9'=0x39
            }
            "Space"    => 0x20,
            "Enter"    => 0x0D,
            "Tab"      => 0x09,
            "Escape"   => 0x1B,
            "Backspace"=> 0x08,
            "Delete"   => 0x2E,
            "Insert"   => 0x2D,
            "Home"     => 0x24,
            "End"      => 0x23,
            "PageUp"   => 0x21,
            "PageDown" => 0x22,
            "ArrowUp"  => 0x26,
            "ArrowDown"=> 0x28,
            "ArrowLeft"=> 0x25,
            "ArrowRight"=>0x27,
            "F1" => 0x70, "F2" => 0x71, "F3" => 0x72, "F4"  => 0x73,
            "F5" => 0x74, "F6" => 0x75, "F7" => 0x76, "F8"  => 0x77,
            "F9" => 0x78, "F10"=> 0x79, "F11"=> 0x7A, "F12" => 0x7B,
            _ => return None,
        };
        return Some(vk);
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn start(hotkey: &str, app_handle: AppHandle) -> Result<(), String> {
    use tauri::Manager;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    // Stop any active release-polling thread
    RELEASE_POLLING.store(false, Ordering::SeqCst);

    // Unregister the previous shortcut
    {
        let mgr = app_handle.global_shortcut();
        if let Ok(mut guard) = CURRENT_SHORTCUT.lock() {
            if let Some(ref old) = *guard {
                let _ = mgr.unregister(old.as_str());
            }
            *guard = None;
        }
    }

    let shortcut_str = to_shortcut_str(hotkey)?;
    let vk_code = hotkey_to_vk(hotkey);

    {
        let app_cb = app_handle.clone();
        app_handle
            .global_shortcut()
            .on_shortcut(shortcut_str.as_str(), move |_app, _shortcut, _event| {
                // RegisterHotKey only fires on key-down. We treat every callback as a press.
                // Guard against double-fires while recording is already active.
                if RELEASE_POLLING
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    log::info!("Hotkey pressed");
                    let _ = app_cb.emit("hotkey-pressed", ());

                    // Spawn a lightweight thread that polls for key release.
                    // GetAsyncKeyState only reads the state of the specific key —
                    // it does not intercept or block any keyboard input.
                    let app_poll = app_cb.clone();
                    let vk = vk_code.unwrap_or(0);
                    std::thread::Builder::new()
                        .name("hotkey-release-poll".into())
                        .spawn(move || {
                            loop {
                                if !RELEASE_POLLING.load(Ordering::SeqCst) {
                                    break;
                                }
                                let still_held = vk > 0
                                    && (unsafe { GetAsyncKeyState(vk) } & (0x8000u16 as i16) != 0);
                                if !still_held {
                                    RELEASE_POLLING.store(false, Ordering::SeqCst);
                                    log::info!("Hotkey released");
                                    let _ = app_poll.emit("hotkey-released", ());
                                    break;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                        })
                        .ok();
                }
            })
            .map_err(|e| format!("Failed to register shortcut '{shortcut_str}': {e}"))?;
    }

    if let Ok(mut guard) = CURRENT_SHORTCUT.lock() {
        *guard = Some(shortcut_str);
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn stop() {
    // Stop release polling. The OS automatically unregisters RegisterHotKey
    // shortcuts when the process exits, so no explicit unregister is needed here.
    RELEASE_POLLING.store(false, Ordering::SeqCst);
    if let Ok(mut guard) = CURRENT_SHORTCUT.lock() {
        *guard = None;
    }
}

#[cfg(not(target_os = "macos"))]
pub fn update(hotkey: &str, app_handle: AppHandle) -> Result<(), String> {
    start(hotkey, app_handle)
}

/// System key capture fallback. Not needed on Windows since the settings window
/// captures keydown events directly via the browser DOM. Returns an error so
/// the frontend falls back gracefully to its own capture mode.
#[cfg(not(target_os = "macos"))]
pub async fn capture_next_key() -> Result<CapturedKey, String> {
    Err("Use the keyboard capture above — press your desired keys in the box.".to_string())
}
