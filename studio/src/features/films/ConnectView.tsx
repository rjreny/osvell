import { useState } from "react";
import { importExportZip, syncSelf } from "../../platform/filmLibrary";
import { pickExportZipPath } from "../../platform/files";
import { log } from "../../platform/log";

export function ConnectView({
  username,
  onUsername,
  onStatus,
  onConnected: _onConnected,
}: {
  username: string;
  onUsername: (name: string) => void;
  onStatus: (s: string) => void;
  onConnected: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function connectRss() {
    const name = username.trim().replace(/^@/, "");
    if (!/^[a-zA-Z0-9_]+$/.test(name)) {
      setError("Use your Letterboxd username (letters, numbers, underscore).");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await syncSelf(name);
      onUsername(name);
      onStatus(`Syncing @${name} in the background`);
    } catch (err) {
      log("error", "rss connect failed", err);
      setError("Could not reach that public diary. Check the username.");
    } finally {
      setBusy(false);
    }
  }

  async function importExport() {
    setBusy(true);
    setError(null);
    try {
      const path = await pickExportZipPath();
      if (!path) return;
      await importExportZip(path);
      onStatus("Importing ZIP in the background — you can keep using Osvell");
    } catch (err) {
      log("error", "export import failed", err);
      setError("Could not read that export. Use an official Letterboxd ZIP.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="connect">
      <p className="hero-cast">Your log</p>
      <h1>Bring your Letterboxd life in.</h1>
      <p className="lede">
        Osvell keeps every diary event: rewatches, rating changes, and overlapping exports. Not just a
        collapsed film count.
      </p>
      <label>
        Username
        <input
          value={username}
          onChange={(e) => onUsername(e.target.value)}
          placeholder="letterboxd username"
          autoCapitalize="off"
          spellCheck={false}
        />
      </label>
      <div className="connect-actions">
        <button type="button" className="play-btn" disabled={busy} onClick={() => void connectRss()}>
          {busy ? "Connecting…" : "Connect public diary"}
        </button>
        <button type="button" className="ghost-pill" disabled={busy} onClick={() => void importExport()}>
          Import export ZIP
        </button>
      </div>
      {error ? <p className="form-error">{error}</p> : null}
      <p className="hint">
        Full history: Letterboxd Settings, Import & Export, download ZIP. After you connect, Osvell
        keeps your public diary RSS up to date on its own — same feed Letterboxd publishes for RSS
        readers, about once an hour. Re-import a fresh export ZIP to include ratings and reviews
        that were added without a diary log.
      </p>
    </div>
  );
}
