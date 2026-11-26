# GPUI Session and Credential Implementation Plan

> Use the existing Tauri behaviors and local fixtures as the compatibility reference.

**Goal** GPUI sends requests with workspace cookies, authenticated proxies, and OAuth 2.0, using the same Rust security and protocol code as Tauri.

**Design** `docs/superpowers/specs/2026-11-26-gpui-parity-design.md`

- [x] Move the existing cookie jar, OS-backed encrypted credentials, token exchange, and loopback OAuth capture into `postlite-core`. Keep the desktop commands as thin adapters and preserve all 81 previous tests across the two crates.
- [x] Use a lazily initialized session per GPUI profile. Read and write GPUI-owned data with a separate credential namespace. Keep read-only runs and unavailable keyrings usable with memory-only cookies. Core tests cover encryption and restart with an injected key store. GPUI tests check profile isolation and explicit Cookie header precedence.
- [x] Add the GPUI cookie drawer and manual proxy settings. Keep proxy credentials out of workspace JSON. Test proxy auth and bypass against loopback fixtures, and verify cookie replay and deletion through the Linux/XWayland UI.
- [x] Resolve OAuth owners through request, folder, and collection inheritance. Implement client credentials and authorization code with PKCE, per-config token caching, refresh, browser callback, cancellation, and manual redirect entry. Test successful and failed token exchange through Send.
- [x] Verify both Linux release builds and the GPUI workbench smoke tests. Launch the workbench over native Wayland with X11 disabled.
- [ ] Validate in-panel HTML/PDF rendering on native Wayland. Wry's [child-window API](https://docs.rs/wry/latest/wry/struct.WebViewBuilder.html#method.build_as_child) supports X11. Its [Wayland-capable API](https://docs.rs/wry/0.57.1/wry/trait.WebViewBuilderExtUnix.html#tymethod.build_gtk) takes a GTK container. The pinned GPUI workbench supplies a Wayland surface handle, so attaching a GTK webview needs a separate integration proof.
