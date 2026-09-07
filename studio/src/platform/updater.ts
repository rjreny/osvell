import { invoke } from "@tauri-apps/api/core";
import { check } from "@tauri-apps/plugin-updater";
import { log } from "./log";
import { relaunchApp } from "./devReload";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "installing"
  | "relaunching"
  | "error";

export type UpdateProgress = {
  phase: UpdatePhase;
  label: string;
  percent: number | null;
  version: string | null;
  error: string | null;
};

export type UpdateCheckResult = {
  available: boolean;
  version: string | null;
  notes: string | null;
  error: string | null;
  message: string | null;
  signingConfigured: boolean;
};

type UpdatePreflight = {
  signing_configured: boolean;
  endpoint: string;
  http_status: number | null;
  reachable: boolean;
  message: string;
  update_available: boolean;
  available_version: string | null;
};

function hasTauri(): boolean {
  return Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);
}

function friendlyUpdateError(err: unknown): string {
  const message = String(err);
  if (message.includes("Failed to fetch dynamically imported module")) {
    return "Update check failed in dev mode — restart the app or use the installed release build.";
  }
  if (message.includes("Failed to fetch") || message.includes("failed to fetch")) {
    return "Could not reach the update server. Check your internet connection.";
  }
  if (message.includes("pubkey") || message.includes("public key")) {
    return "Update signing is not configured yet";
  }
  return message;
}

function fromPreflight(preflight: UpdatePreflight): UpdateCheckResult {
  if (!preflight.reachable) {
    return {
      available: false,
      version: null,
      notes: null,
      error: preflight.message,
      message: null,
      signingConfigured: preflight.signing_configured,
    };
  }

  return {
    available: preflight.update_available,
    version: preflight.available_version,
    notes: null,
    error: null,
    message: preflight.message,
    signingConfigured: preflight.signing_configured,
  };
}

/** Check release feed via Rust HTTP (reliable in dev and production). */
export async function checkAppUpdate(): Promise<UpdateCheckResult> {
  if (!hasTauri()) {
    return {
      available: false,
      version: null,
      notes: null,
      error: "Updates run in the installed app",
      message: null,
      signingConfigured: false,
    };
  }

  try {
    const preflight = await invoke<UpdatePreflight>("update_preflight");
    return fromPreflight(preflight);
  } catch (err) {
    log("warn", "update check failed", err);
    return {
      available: false,
      version: null,
      notes: null,
      error: friendlyUpdateError(err),
      message: null,
      signingConfigured: false,
    };
  }
}

export async function downloadAndInstallUpdate(
  onProgress: (progress: UpdateProgress) => void,
): Promise<void> {
  if (!hasTauri()) {
    onProgress({
      phase: "error",
      label: "Updates run in the installed app",
      percent: null,
      version: null,
      error: "Not running in Osvell",
    });
    return;
  }

  if (import.meta.env.DEV) {
    onProgress({
      phase: "error",
      label: "Install updates from the release build",
      percent: null,
      version: null,
      error: "Dev builds cannot install NSIS updates. Install Osvell from the GitHub release, then use Update there.",
    });
    return;
  }

  onProgress({
    phase: "checking",
    label: "Checking for updates…",
    percent: null,
    version: null,
    error: null,
  });

  try {
    const preflight = await invoke<UpdatePreflight>("update_preflight");
    if (!preflight.signing_configured) {
      onProgress({
        phase: "error",
        label: "Update signing is not configured",
        percent: null,
        version: null,
        error: preflight.message,
      });
      return;
    }

    const update = await check();
    if (!update) {
      onProgress({
        phase: "idle",
        label: "You're up to date",
        percent: 100,
        version: null,
        error: null,
      });
      return;
    }

    onProgress({
      phase: "downloading",
      label: `Downloading ${update.version}…`,
      percent: 0,
      version: update.version,
      error: null,
    });

    let downloaded = 0;
    let total = 0;

    await update.downloadAndInstall((event) => {
      switch (event.event) {
        case "Started":
          total = event.data.contentLength ?? 0;
          onProgress({
            phase: "downloading",
            label: `Downloading ${update.version}…`,
            percent: total > 0 ? 0 : null,
            version: update.version,
            error: null,
          });
          break;
        case "Progress":
          downloaded += event.data.chunkLength;
          const pct = total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
          onProgress({
            phase: "downloading",
            label: `Downloading ${update.version}…`,
            percent: pct,
            version: update.version,
            error: null,
          });
          break;
        case "Finished":
          onProgress({
            phase: "installing",
            label: "Finishing update — Osvell will reopen automatically…",
            percent: 100,
            version: update.version,
            error: null,
          });
          break;
      }
    });

    onProgress({
      phase: "relaunching",
      label: "Restarting Osvell…",
      percent: 100,
      version: update.version,
      error: null,
    });

    await relaunchApp();
  } catch (err) {
    log("error", "update install failed", err);
    onProgress({
      phase: "error",
      label: "Update failed",
      percent: null,
      version: null,
      error: friendlyUpdateError(err),
    });
  }
}

/** Connectivity smoke via Rust HTTP (WebView fetch is blocked for release URLs). */
export async function smokeCheckUpdate(): Promise<string> {
  if (!hasTauri()) {
    return "Updates run in the installed app";
  }
  try {
    const preflight = await invoke<UpdatePreflight>("update_preflight");
    return preflight.message;
  } catch (err) {
    const note = `Update check failed (${friendlyUpdateError(err)})`;
    log("warn", "update check failed", err);
    return note;
  }
}
