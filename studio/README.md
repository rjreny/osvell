# Osvell

Developer guide for Osvell. Looking to use the app? Start with the [product README](../README.md) or [download the latest Windows release](https://github.com/rjreny/osvell/releases/latest).

The product lives in this directory. The `../prototypes/` shells are historical experiments. Osvell uses Tauri 2; features talk to `src/platform` only and must not import `@tauri-apps/*` directly. The lint configuration enforces that boundary.

Install Node.js/npm and the Windows Tauri build prerequisites (Rust, Microsoft C++ Build Tools, and WebView2), then run these commands from this directory:

```bash
npm install
npm run lint
npm run dev:manual   # recommended: manual Update button, no auto-reload
npm run tauri dev    # legacy: Rust changes still auto-restart
npm run tauri build
```

While developing, use `npm run dev:manual` so code changes queue behind an **Update** button in the status bar instead of hot-reloading the UI. Requires `cargo install cargo-watch`. Production updates in Settings also wait for you to click **Update**, show download/install progress, then restart once.

NSIS is per-user. After any change that should reach the installed app, bump the version, push `master`, and push a `v*` tag so release CI can cut a signed NSIS installer and `latest.json` for auto-update. Do not wait for a reminder — a push without a `v*` tag does not update the installed app.

Release CI needs the `TAURI_SIGNING_PRIVATE_KEY` repository secret. Keep the signing key out of source control. The updater feed must be publicly reachable; pushes to `master` warm the Rust build cache, while `v*` tags publish the installer and updater manifest. Documentation-only changes do not require a version bump or installer release.

Repository: [rjreny/osvell](https://github.com/rjreny/osvell).

Osvell was previously named Studio. See [rename compatibility](../docs/osvell-rename.md) before changing application identifiers, storage names or the packaged executable filename.
