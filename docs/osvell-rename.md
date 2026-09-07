# Osvell rename compatibility

Osvell 0.14.1 is the continuation of Studio, not a separate app or library.

## Public identity

- Product, window title, UI messages, npm package and Rust crate: Osvell / `osvell`.
- Repository and new updater endpoint: `https://github.com/rjreny/osvell`.
- The existing signing public key and repository signing secret are retained.
- GitHub redirects the old repository and release URLs. Do not create a replacement
  `rjreny/studio` repository: existing installed clients still use its updater URL.

## Intentionally stable contracts

- Tauri identifier `com.rjreny.studio`: preserves the application data directory,
  WebView identity, window state and Windows taskbar grouping.
- `studio.db`, `studio.json`, `studio.log` and Credential Manager service `studio`:
  preserve viewing history, preferences, logs and saved TMDB/OpenRouter credentials.
- Packaged executable `studio.exe`: preserves existing pinned taskbar shortcuts.
  `mainBinaryName` explicitly keeps this filename while the Rust crate is `osvell`.
- Internal `studio-job` events, dev reload endpoints, `STUDIO_*` benchmark variables
  and signing-key filename remain compatible with existing tools.
- The source directory stays `studio/`, so existing local checkouts, build commands
  and in-progress branches continue to work. Movie-production `studio` fields are
  domain terminology and are unrelated to the former product name.
- Historical design plans, prototype names and screenshot filenames are archival.

## Windows upgrade

The standard Tauri NSIS template is retained. Installer hooks recognize a per-user
Studio installation only when the saved publisher is `rjreny` and its executable
exists. Both interactive installs and silent updater installs reuse that directory.
The existing executable is closed before replacement. After installation writes
Osvell's uninstaller and registry entry, matching Start menu/Desktop shortcuts are
renamed and the old Studio registration is removed. No application data is moved,
copied, reset or deleted by these hooks. Fresh installs use the Osvell directory.

## Release verification

Build and test the frontend and Rust crate. Compile the NSIS installer and verify
both fresh-install and legacy-install hook paths in an isolated Windows fixture.
After renaming GitHub, verify both the old and new `latest.json` URLs return the
same signed Osvell version, and verify the manifest's Windows installer URL.

Run `powershell -ExecutionPolicy Bypass -File scripts/verify-rename.ps1` from
`studio/` after a Windows bundle build. This compiles the actual hooks into a
silent fixture with a unique scratch registry namespace, fixture-only executable
name and redirected shortcut folders. It exercises fresh installs, publisher and
missing-binary guards, old install/output paths, registry and shortcut migration,
preserved sample history, and repeated upgrades. It does not run the real app.

Baseline comparison for 0.14.1: the frontend production build and 47 frontend
tests pass. Three existing RecsView tests fail on ambiguous duplicate-title queries.
Rust has 407 passing tests, nine existing Taste failures and 15 ignored live-data
tests. ESLint reports eight existing errors. The untouched 0.14.0 release commit
reproduces these same failures; this rename does not change recommendation logic.
