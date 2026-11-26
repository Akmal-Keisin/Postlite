# Postlite GPUI prototype

This prototype tests startup and idle memory with a GPUI window. Enter an HTTP or HTTPS URL and press **Send** or **Enter** to display the GET response, status, latency, and size. Small JSON responses are formatted automatically. Postlite reads at most 2,000,001 decoded response bytes and uses the extra byte to detect whether the 2,000,000-byte preview was truncated.

The prototype covers a single request at a time. The Tauri app remains the complete client for collections, saved workspaces, authentication, and HTML and PDF previews.

```sh
cargo run --release --locked --manifest-path src-gpui/Cargo.toml
cargo test --release --locked --manifest-path src-gpui/Cargo.toml
```

The HTTP client is created off the UI thread on the first request and reused for later requests. The response editor is also allocated on demand. The lockfile selects compatible `libc` and `mio` versions because GPUI 0.2.2's archive dependency references the former Linux `libc::ENOATTR` constant.

## Benchmark

Run from the repository root on a Linux desktop with an X11 or XWayland display and `xdotool` installed.

```sh
cargo build --release --locked --manifest-path src-tauri/Cargo.toml
cargo build --release --locked --manifest-path src-gpui/Cargo.toml
python3 .verify/gpui-benchmark.py --runs 7
```

Each run uses a temporary app profile. The harness discards one warmup per app and alternates launch order. It records time to X11 window mapping and time to first visible content separately. The content check samples the top-left of the window for pixels from text and controls. The harness measures proportional set size (PSS) across the app's process tree two seconds after content appears. The current Tauri build has more features than the GPUI prototype, so the results describe these two builds at launch.

Seven warm launch trials on Linux/XWayland with an AMD Ryzen 5 6600H gave these medians.

| Metric | Tauri | GPUI |
| --- | ---: | ---: |
| Window mapped | 83.4 ms | 125.7 ms |
| First visible content | 548.7 ms | 186.9 ms |
| Idle process-tree PSS | 288.8 MiB | 55.3 MiB |
