import { invoke } from "@tauri-apps/api/core";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { getSetting, setSetting } from "./settings";

export type ResidentPrefs = {
  launchAtLogin: boolean;
  startMinimized: boolean;
  closeMinimizes: boolean;
};

const listeners = new Set<(prefs: ResidentPrefs) => void>();

export function normalizeResidentPrefs(prefs: ResidentPrefs): ResidentPrefs {
  const launchAtLogin = Boolean(prefs.launchAtLogin);
  return {
    launchAtLogin,
    startMinimized: launchAtLogin && Boolean(prefs.startMinimized),
    closeMinimizes: Boolean(prefs.closeMinimizes),
  };
}

export function subscribeResident(listener: (prefs: ResidentPrefs) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function publish(prefs: ResidentPrefs) {
  for (const listener of listeners) listener(prefs);
}

function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function loadResidentPrefs(): Promise<ResidentPrefs> {
  const startMinimized = Boolean(await getSetting<boolean>("startMinimized"));
  const closeMinimizes = Boolean(await getSetting<boolean>("closeMinimizes"));
  let launchAtLogin = Boolean(await getSetting<boolean>("launchAtLogin"));
  if (inTauri()) {
    try {
      launchAtLogin = await isEnabled();
    } catch {
      /* keep the stored value when the plugin is unavailable */
    }
  }
  return normalizeResidentPrefs({ launchAtLogin, startMinimized, closeMinimizes });
}

export async function saveResidentPrefs(prefs: ResidentPrefs): Promise<void> {
  const next = normalizeResidentPrefs(prefs);
  if (inTauri()) {
    if (next.launchAtLogin) await enable();
    else await disable();
    await invoke("set_resident_prefs", {
      closeMinimizes: next.closeMinimizes,
      startMinimized: next.startMinimized,
    });
  }
  await setSetting("launchAtLogin", next.launchAtLogin);
  await setSetting("startMinimized", next.startMinimized);
  await setSetting("closeMinimizes", next.closeMinimizes);
  publish(next);
}
