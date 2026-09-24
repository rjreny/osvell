import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import { type Accent, type Theme } from "../../core/types";
import { pickExportZipPath } from "../../platform/files";
import {
  formatLibrarySummary,
  formatBytes,
  formatEnrich,
  formatImport,
  formatRssSyncAt,
  importExportZip,
  importGetDiagnostics,
  syncFeeds,
  tmdbClearKey,
  tmdbEnrich,
  tmdbKeyStatus,
  tmdbSetKey,
  tasteClearKey,
  tasteKeyStatus,
  tasteSetKey,
  tasteSetModel,
  tasteSetWeb,
} from "../../platform/filmLibrary";
import type { EnrichReport, ImportResult, InstallInfo, JobProgress, LibraryCoverage, TasteKeyStatus, TmdbKeyStatus } from "../../platform/types/film";
import {
  getInstallInfo,
  installKindLabel,
  launchUninstaller,
  openDataFolder,
  openLogFile,
  resetAppData,
} from "../../platform/install";
import { log } from "../../platform/log";
import { loadResidentPrefs, saveResidentPrefs, type ResidentPrefs } from "../../platform/resident";
import {
  checkAppUpdate,
  downloadAndInstallUpdate,
  type UpdateProgress,
} from "../../platform/updater";
import { UpdateOverlay } from "../../app/shell/UpdateOverlay";
import { TasteModelModal } from "./TasteModelModal";

const idleProgress: UpdateProgress = {
  phase: "idle",
  label: "Ready",
  percent: null,
  version: null,
  error: null,
};

const emptyResident: ResidentPrefs = {
  launchAtLogin: false,
  startMinimized: false,
  closeMinimizes: false,
};

function PrefSwitch({
  id,
  label,
  hint,
  checked,
  disabled,
  nested,
  onChange,
}: {
  id: string;
  label: string;
  hint: string;
  checked: boolean;
  disabled?: boolean;
  nested?: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className={`settings-switch${nested ? " is-nested" : ""}${disabled && nested ? " is-locked" : ""}`}>
      <div>
        <p id={id} className="settings-switch-label">{label}</p>
        <p className="hint">{hint}</p>
      </div>
      <button
        type="button"
        className="settings-toggle"
        role="switch"
        aria-checked={checked}
        aria-labelledby={id}
        disabled={disabled}
        onClick={() => onChange(!checked)}
      >
        <span />
      </button>
    </div>
  );
}

export function SettingsView({
  theme,
  accent,
  version,
  username,
  coverage,
  lastRssSyncAt,
  rssPausedUntil,
  onTheme,
  onAccent,
  onUsername,
  onStatus,
  onRefresh,
}: {
  theme: Theme;
  accent: Accent;
  version: string;
  username: string;
  coverage: LibraryCoverage | null;
  lastRssSyncAt?: string | null;
  rssPausedUntil?: string | null;
  onTheme: (t: Theme) => void;
  onAccent: (a: Accent) => void;
  onUsername: (name: string) => void;
  onStatus: (text: string) => void;
  onRefresh: () => Promise<void>;
}) {
  const [updateNote, setUpdateNote] = useState("Not checked");
  const [signingConfigured, setSigningConfigured] = useState(true);
  const [pendingVersion, setPendingVersion] = useState<string | null>(null);
  const [updateOpen, setUpdateOpen] = useState(false);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress>(idleProgress);
  const [keyStatus, setKeyStatus] = useState<TmdbKeyStatus | null>(null);
  const [tasteStatus, setTasteStatus] = useState<TasteKeyStatus | null>(null);
  const [tasteModelOpen, setTasteModelOpen] = useState(false);
  const [replacing, setReplacing] = useState(false);
  const [tasteReplacing, setTasteReplacing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [keyInput, setKeyInput] = useState("");
  const [tasteKeyInput, setTasteKeyInput] = useState("");
  const [diagnostics, setDiagnostics] = useState<string[]>([]);
  const [installInfo, setInstallInfo] = useState<InstallInfo | null>(null);
  const [lastImport, setLastImport] = useState<ImportResult | null>(null);
  const [lastEnrich, setLastEnrich] = useState<EnrichReport | null>(null);
  const [syncDetailsOpen, setSyncDetailsOpen] = useState(false);
  const [resident, setResident] = useState<ResidentPrefs>(emptyResident);
  const [residentReady, setResidentReady] = useState(false);
  const residentRef = useRef(resident);
  residentRef.current = resident;
  const tasteModelChangeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let alive = true;
    void loadResidentPrefs().then((prefs) => {
      if (!alive) return;
      setResident(prefs);
      setResidentReady(true);
    });
    return () => {
      alive = false;
    };
  }, []);

  async function updateResident(patch: Partial<ResidentPrefs>) {
    const previous = residentRef.current;
    const next = {
      ...previous,
      ...patch,
      startMinimized:
        (patch.launchAtLogin ?? previous.launchAtLogin) &&
        (patch.startMinimized ?? previous.startMinimized),
    };
    if (patch.launchAtLogin === false) next.startMinimized = false;
    residentRef.current = next;
    setResident(next);
    try {
      await saveResidentPrefs(next);
    } catch (err) {
      log("warn", "startup preferences failed", err);
      onStatus("Could not update startup settings");
      const restored = await loadResidentPrefs().catch(() => previous);
      residentRef.current = restored;
      setResident(restored);
    }
  }

  useEffect(() => {
    void (async () => {
      try {
        const status = await tmdbKeyStatus();
        setKeyStatus(status);
      } catch {
        /* dev without tauri */
      }
      try {
        setTasteStatus(await tasteKeyStatus());
      } catch {
        /* dev without tauri */
      }
      try {
        const d = await importGetDiagnostics();
        setDiagnostics(d.warnings);
      } catch {
        /* dev without tauri */
      }
      try {
        setInstallInfo(await getInstallInfo());
      } catch (err) {
        log("warn", "install info unavailable", err);
      }
    })();
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<JobProgress>("studio-job", (event) => {
      const next = event.payload;
      if (next.import) setLastImport(next.import);
      if (next.enrich) {
        setLastEnrich(next.enrich);
      }
      if (next.done && next.import) {
        onStatus(formatImport(next.import));
      }
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [onStatus]);

  async function checkUpdates() {
    const result = await checkAppUpdate();
    setSigningConfigured(result.signingConfigured);
    if (result.error) {
      setUpdateNote(result.error);
      setPendingVersion(null);
      return;
    }
    if (result.available) {
      setPendingVersion(result.version);
      setUpdateNote(`Update available: ${result.version}`);
      return;
    }
    setPendingVersion(null);
    setUpdateNote(result.message ?? "You're up to date");
  }

  async function installUpdate() {
    setUpdateOpen(true);
    setUpdateProgress({
      phase: "checking",
      label: "Preparing update…",
      percent: null,
      version: pendingVersion,
      error: null,
    });
    await downloadAndInstallUpdate(setUpdateProgress);
  }

  async function refreshDiary() {
    try {
      setBusy(true);
      const started = await syncFeeds(true);
      onStatus(
        started
          ? "Refreshing public diary RSS in the background"
          : "No public diary to refresh yet — add your username first",
      );
      await onRefresh();
    } catch (err) {
      log("warn", "diary rss sync skipped", err);
      onStatus("Could not refresh diary RSS right now");
    } finally {
      setBusy(false);
    }
  }

  async function importExport() {
    try {
      const path = await pickExportZipPath();
      if (!path) return;
      setBusy(true);
      await importExportZip(path);
      onStatus("Importing ZIP in the background — you can keep using Osvell");
      setBusy(false);
    } catch (err) {
      log("error", "settings import failed", err);
      onStatus("Import failed — library unchanged");
    } finally {
      setBusy(false);
    }
  }

  async function saveKey() {
    if (!keyInput.trim()) return;
    try {
      setBusy(true);
      const status = await tmdbSetKey(keyInput.trim());
      const confirmed = status.stored ? await tmdbKeyStatus().catch(() => status) : status;
      setKeyStatus(confirmed);
      setKeyInput("");
      if (confirmed.valid !== true || !confirmed.stored) {
        onStatus(
          confirmed.lastError ?? "TMDB accepted this key, but Windows did not keep it — it was not saved",
        );
        return;
      }
      setReplacing(false);
      onStatus("TMDB accepted this key — matching posters in the background");
      await tmdbEnrich();
    } catch (err) {
      log("error", "tmdb key save failed", err);
      onStatus("Could not store TMDB key or fetch posters");
    } finally {
      setBusy(false);
    }
  }

  async function runEnrich() {
    try {
      setBusy(true);
      await tmdbEnrich();
      onStatus("Matching TMDB in the background — you can keep browsing");
    } catch (err) {
      log("error", "enrich failed", err);
      onStatus("Poster fetch failed — open studio.log for details");
    } finally {
      setBusy(false);
    }
  }

  async function saveTasteKey() {
    if (!tasteKeyInput.trim()) return;
    try {
      setBusy(true);
      const status = await tasteSetKey(tasteKeyInput.trim());
      setTasteStatus(status);
      setTasteKeyInput("");
      if (status.valid !== true || !status.stored) {
        onStatus(status.lastError ?? "OpenRouter did not accept this key");
        return;
      }
      setTasteReplacing(false);
      onStatus("OpenRouter key saved. Taste is ready.");
    } catch (err) {
      log("error", "taste key save failed", err);
      onStatus("Could not store OpenRouter key");
    } finally {
      setBusy(false);
    }
  }

  async function pickTasteModel(model: string) {
    try {
      setTasteStatus(await tasteSetModel(model));
    } catch (err) {
      log("warn", "taste model save failed", err);
    }
  }

  async function pickTasteWeb(enabled: boolean) {
    try {
      setTasteStatus(await tasteSetWeb(enabled));
    } catch (err) {
      log("warn", "taste web save failed", err);
    }
  }

  function closeTasteModelModal() {
    setTasteModelOpen(false);
    tasteModelChangeRef.current?.focus();
  }

  const tasteModelId = tasteStatus?.model ?? "deepseek/deepseek-v4-pro-0813";
  const selectedTasteModel = tasteStatus?.models.find((model) => model.id === tasteModelId);
  const tasteModelLabel =
    selectedTasteModel?.label ??
    (tasteModelId === "deepseek/deepseek-v4-pro-0813"
      ? "DeepSeek V4 Pro 0813"
      : tasteModelId);
  const tasteModelBlurb =
    selectedTasteModel?.blurb ?? "Recommended for nuanced, evidence-grounded film picks.";
  const keyConnected = Boolean(keyStatus?.stored && keyStatus.valid === true && !replacing);
  const tasteConnected = Boolean(tasteStatus?.stored && tasteStatus.valid !== false && !tasteReplacing);
  const letterboxdConnected = Boolean(
    username.trim() &&
      (lastRssSyncAt ||
        (coverage &&
          (coverage.source !== "none" || coverage.fullHistoryAvailable))),
  );

  async function confirmResetData() {
    const ok = await ask(
      "This removes your library, friends, posters, and saved preferences from this device. It cannot be undone.",
      { title: "Reset Osvell?", kind: "warning", okLabel: "Reset everything", cancelLabel: "Cancel" },
    );
    if (!ok) return;
    try {
      await resetAppData();
    } catch (err) {
      log("error", "reset failed", err);
      onStatus("Could not reset Osvell data");
    }
  }

  async function runUninstaller() {
    try {
      await launchUninstaller();
      onStatus("Uninstaller opened — your library data stays until you reset it or remove the data folder");
    } catch (err) {
      log("warn", "uninstaller unavailable", err);
      onStatus("No uninstaller for this build — remove dev shortcuts manually or use Installed apps in Windows Settings");
    }
  }

  return (
    <div className="settings-page page-pad">
      <div className="settings-content">
        <header className="page-head">
          <div>
            <h1>Settings</h1>
            <p className="muted">Library, appearance, recommendations, and this PC</p>
          </div>
        </header>
        <div className="settings-grid">
          <section className="settings-quadrant settings-library-panel">
            <h2>Library</h2>
            <p className="hint">Your Letterboxd history, poster matching, and diary refresh.</p>
            <div className="settings-subgroup">
              <header className="settings-subgroup-head">
                <h3>
                  <label htmlFor="settings-user">Letterboxd</label>
                </h3>
                <p className={`hint settings-letterboxd-status${letterboxdConnected ? "" : " is-bad"}`}>
                  {letterboxdConnected ? "Connected" : "Not connected"}
                </p>
              </header>
              <input
                id="settings-user"
                value={username}
                onChange={(e) => onUsername(e.target.value)}
                placeholder="username"
              />
              <div className="settings-actions">
                <button
                  type="button"
                  className="primary"
                  disabled={busy || !username.trim()}
                  onClick={() => void refreshDiary()}
                >
                  {busy ? "Working…" : "Sync now"}
                </button>
                <button type="button" className="ghost-pill" disabled={busy} onClick={() => void importExport()}>
                  {busy ? "Working…" : "Import history"}
                </button>
              </div>
              <button type="button" className="text-btn settings-tertiary-action" disabled={busy} onClick={() => void runEnrich()}>
                Match posters <span aria-hidden="true">›</span>
              </button>
              <div className="settings-sync-disclosure">
                <button
                  type="button"
                  className="text-btn"
                  aria-expanded={syncDetailsOpen}
                  onClick={() => setSyncDetailsOpen((open) => !open)}
                >
                  How syncing works <span aria-hidden="true">›</span>
                </button>
                <span className="hint">Last refresh: {formatRssSyncAt(lastRssSyncAt)}</span>
                {syncDetailsOpen ? (
                  <p className="hint settings-sync-details">
                    Osvell refreshes your public Letterboxd diary RSS about once an hour while the app is
                    open, and when you launch it. Import a fresh Letterboxd export ZIP whenever you want to
                    add ratings and reviews that were not diary logs. Same official feeds RSS readers use —
                    no site scraping.
                  </p>
                ) : null}
              </div>
              {rssPausedUntil ? (
                <p className="hint">
                  Paused until {formatRssSyncAt(rssPausedUntil)} because Letterboxd asked us to wait.
                </p>
              ) : null}
              {diagnostics.map((w) => (
                <p key={w} className="hint">
                  {w}
                </p>
              ))}
              {lastImport ? <p className="hint">{formatImport(lastImport)}</p> : null}
            </div>
            <div className="settings-subgroup">
              <header className="settings-subgroup-head">
                <h3>
                  <label htmlFor="settings-tmdb">TMDB</label>
                </h3>
                {keyConnected ? <p className="key-status">Configured</p> : null}
              </header>
              {keyConnected ? null : (
                <input
                  id="settings-tmdb"
                  value={keyInput}
                  onChange={(e) => setKeyInput(e.target.value)}
                  placeholder={keyStatus?.stored ? "Paste a replacement key" : "v3 key from themoviedb.org"}
                  spellCheck={false}
                  disabled={busy}
                />
              )}
              {keyStatus?.stored && keyStatus.valid === false ? (
                <p className="key-status is-bad">{keyStatus.lastError ?? "TMDB rejected this key."}</p>
              ) : null}
              <div className="settings-actions">
                {!keyConnected ? (
                  <button type="button" className="ghost-pill" disabled={busy || !keyInput.trim()} onClick={() => void saveKey()}>
                    Save key
                  </button>
                ) : (
                  <button type="button" className="text-btn" disabled={busy} onClick={() => setReplacing(true)}>
                    Change
                  </button>
                )}
                <button
                  type="button"
                  className="text-btn"
                  disabled={busy || !keyStatus?.stored}
                  onClick={() =>
                    void tmdbClearKey().then((status) => {
                      setKeyStatus(status);
                      setReplacing(false);
                      onStatus("TMDB key removed");
                    })
                  }
                >
                  Remove
                </button>
              </div>
              {lastEnrich ? <p className="hint">{formatEnrich(lastEnrich)}</p> : null}
            </div>
          </section>

          <section className="settings-quadrant">
            <h2>Appearance</h2>
            <p className="hint">Choose the theme and accent Osvell uses on this PC.</p>
            <div className="settings-subgroup">
              <h3>Theme</h3>
              <div className="theme-preview-row" role="radiogroup" aria-label="Theme">
                {(["system", "dark", "light"] as const).map((t) => (
                  <button
                    key={t}
                    type="button"
                    role="radio"
                    aria-checked={theme === t}
                    className={`theme-preview is-${t}${theme === t ? " is-on" : ""}`}
                    onClick={() => onTheme(t)}
                  >
                    <span className="theme-preview-surface" aria-hidden="true" />
                    <span>{t[0].toUpperCase() + t.slice(1)}</span>
                  </button>
                ))}
              </div>
            </div>
            <div className="settings-subgroup">
              <h3>Accent</h3>
              <div className="accent-radio-row" role="radiogroup" aria-label="Accent">
                {(["app", "system"] as const).map((a) => (
                  <button
                    key={a}
                    type="button"
                    role="radio"
                    aria-checked={accent === a}
                    className={`accent-radio${accent === a ? " is-on" : ""}`}
                    onClick={() => onAccent(a)}
                  >
                    <span className={`accent-radio-dot is-${a}`} aria-hidden="true" />
                    <span>{a === "app" ? "Osvell blue" : "System"}</span>
                  </button>
                ))}
              </div>
            </div>
          </section>

          <section className="settings-quadrant">
            <h2>Taste</h2>
            <p className="hint">Choose how Taste reads your film history and whether it can check the web.</p>
            <div className="settings-subgroup">
              <header className="settings-subgroup-head">
                <h3>Recommendation model</h3>
                <button
                  ref={tasteModelChangeRef}
                  type="button"
                  className="text-btn settings-taste-change"
                  aria-label="Change recommendation model"
                  disabled={busy || !(tasteStatus?.models.length)}
                  onClick={() => setTasteModelOpen(true)}
                >
                  Change <span aria-hidden="true">›</span>
                </button>
              </header>
              <strong className="settings-model-name">{tasteModelLabel}</strong>
              <p className="hint">{tasteModelBlurb}</p>
            </div>
            <div className="settings-subgroup">
              <PrefSwitch
                id="taste-web"
                label="Web search"
                hint="A few critic-list lookups per run. Caps cost. No multi-model swarm."
                checked={Boolean(tasteStatus?.web)}
                disabled={busy || !tasteStatus}
                onChange={(enabled) => void pickTasteWeb(enabled)}
              />
            </div>
            <div className="settings-subgroup">
              <header className="settings-subgroup-head">
                <h3>
                  <label htmlFor="settings-openrouter">OpenRouter</label>
                </h3>
                {tasteConnected ? <p className="key-status">Configured</p> : null}
              </header>
              {tasteConnected ? null : (
                <input
                  id="settings-openrouter"
                  value={tasteKeyInput}
                  onChange={(e) => setTasteKeyInput(e.target.value)}
                  placeholder={tasteStatus?.stored ? "Paste a replacement key" : "sk-or-... from openrouter.ai/keys"}
                  spellCheck={false}
                  disabled={busy}
                />
              )}
              {tasteStatus?.stored && tasteStatus.valid === false ? (
                <p className="key-status is-bad">{tasteStatus.lastError ?? "OpenRouter rejected this key."}</p>
              ) : null}
              <div className="settings-actions">
                {!tasteConnected ? (
                  <button
                    type="button"
                    className="ghost-pill"
                    disabled={busy || !tasteKeyInput.trim()}
                    onClick={() => void saveTasteKey()}
                  >
                    Save key
                  </button>
                ) : (
                  <button
                    type="button"
                    className="text-btn"
                    aria-label="Change OpenRouter key"
                    disabled={busy}
                    onClick={() => setTasteReplacing(true)}
                  >
                    Change
                  </button>
                )}
                <button
                  type="button"
                  className="text-btn"
                  aria-label="Remove OpenRouter key"
                  disabled={busy || !tasteStatus?.stored}
                  onClick={() =>
                    void tasteClearKey().then((status) => {
                      setTasteStatus(status);
                      setTasteReplacing(false);
                      onStatus("OpenRouter key removed");
                    })
                  }
                >
                  Remove
                </button>
              </div>
            </div>
          </section>

          <section className="settings-quadrant">
            <h2>System</h2>
            <div className="settings-subgroup">
              <h3>Startup</h3>
              <div className="settings-switch-list">
                <PrefSwitch
                  id="open-with-windows"
                  label="Open with Windows"
                  hint="Starts when you sign in, so new diary activity can be picked up before you open Osvell."
                  checked={resident.launchAtLogin}
                  disabled={!residentReady}
                  onChange={(launchAtLogin) => void updateResident({ launchAtLogin })}
                />
                <PrefSwitch
                  id="start-in-tray"
                  label="Start in the tray"
                  hint="Stay in the tray at sign-in instead of opening a window."
                  checked={resident.startMinimized}
                  nested
                  disabled={!residentReady || !resident.launchAtLogin}
                  onChange={(startMinimized) => void updateResident({ startMinimized })}
                />
                <PrefSwitch
                  id="keep-running"
                  label="Keep running when closed"
                  hint="Closing hides Osvell so the next open is immediate. Shift-click close, press Ctrl+Q, or choose Quit in the tray menu to exit."
                  checked={resident.closeMinimizes}
                  disabled={!residentReady}
                  onChange={(closeMinimizes) => void updateResident({ closeMinimizes })}
                />
              </div>
              <p className="hint">
                Osvell always opens on Home. While it is running, including in the tray, it checks Letterboxd about once an hour for new diary and friend activity and backs off if Letterboxd asks it to wait.
              </p>
            </div>
            <section className="settings-subgroup settings-system-device">
              <h3>This PC</h3>
              <p className="hint">Storage, logs, and recovery controls for this installation.</p>
              {installInfo ? (
                <>
                  <p className="hint">
                    {installKindLabel(installInfo.installKind)} · {formatBytes(installInfo.dataBytes)} on disk
                  </p>
                  {coverage ? <p className="hint">{formatLibrarySummary(coverage)}</p> : null}
                  {coverage?.warnings[0] ? <p className="hint">{coverage.warnings[0]}</p> : null}
                  <p className="mono-path">{installInfo.appDataDir}</p>
                </>
              ) : (
                <p className="hint">Storage details are unavailable for this build.</p>
              )}
              <div className="settings-actions">
                <button type="button" className="text-btn" onClick={() => void openDataFolder().catch(() => onStatus("Could not open data folder"))}>
                  Open folder <span aria-hidden="true">›</span>
                </button>
                <button type="button" className="text-btn" onClick={() => void openLogFile().catch(() => onStatus("Could not open studio.log"))}>
                  Log
                </button>
                {installInfo?.uninstallerPath ? (
                  <button type="button" className="text-btn" onClick={() => void runUninstaller()}>
                    Uninstall
                  </button>
                ) : null}
                <button type="button" className="text-btn" onClick={() => void confirmResetData()}>
                  Reset data
                </button>
              </div>
            </section>
            <section className={`settings-subgroup settings-system-updates${pendingVersion ? " is-update-available" : ""}`}>
              <header className="settings-subgroup-head">
                <h3>Updates</h3>
                {pendingVersion ? <span className="settings-update-badge">Update available</span> : null}
              </header>
              <p className="hint">Keep Osvell current with the latest fixes and improvements.</p>
              <p className="settings-system-version">Version {version}</p>
              <div className="update-line">
                <button type="button" className="ghost-pill" onClick={() => void checkUpdates()}>
                  Check for updates
                </button>
                {pendingVersion ? (
                  <button type="button" className="primary" onClick={() => void installUpdate()}>
                    Update to {pendingVersion}
                  </button>
                ) : null}
                <p className="update-note" aria-live="polite">{updateNote}</p>
              </div>
              {import.meta.env.DEV ? (
                <p className="hint">
                  Dev builds cannot install updates. Use the installer from{" "}
                  <a href="https://github.com/rjreny/osvell/releases" target="_blank" rel="noreferrer">
                    GitHub Releases
                  </a>
                  .
                </p>
              ) : null}
              {!signingConfigured && !import.meta.env.DEV ? (
                <p className="hint">Signing is not configured in this build. Reinstall from a signed GitHub release.</p>
              ) : null}
            </section>
          </section>
        </div>
      </div>
      <UpdateOverlay
        open={updateOpen}
        title="Installing update"
        progress={updateProgress}
        onClose={() => {
          setUpdateOpen(false);
          setUpdateProgress(idleProgress);
        }}
      />
      <TasteModelModal
        open={tasteModelOpen}
        models={tasteStatus?.models ?? []}
        selected={tasteModelId}
        disabled={busy}
        onClose={closeTasteModelModal}
        onPick={(id) => void pickTasteModel(id)}
      />
    </div>
  );
}
