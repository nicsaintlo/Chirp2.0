# Known Issues

## Mac

- **Launch on startup is glitchy** — Enabling "launch on startup" on macOS results in unreliable behavior.
- **Hotkey may require re-setting after first launch** — After initial setup, the hotkey may not respond until changed and changed back in settings.
- **Clear history confirmation may not render** — `window.confirm()` can be suppressed in Tauri webview on some macOS versions. v1.2.5 replaces with branded modal.

## Windows

- **Tray menu quit button may not appear** — The "Quit Chirp" menu item is defined in code but may not render on some Windows configurations (likely a Tauri rendering bug).
- **Scroll may stop working after dictation** — Reported in Notepad on Windows 11 24H2. Triggering another dictation restores scrolling. Likely related to keyboard hook lifecycle.

## Cross-platform

- **TAURI_IPC errors on copy button** — (RUST-K) 10 events / 6 users. Copy button can fail when Tauri IPC bridge is unavailable. Under investigation.
- **Window destroy panic** — (RUST-N) Rare fatal crash when window enters Destroyed state during async operations. Under investigation.
