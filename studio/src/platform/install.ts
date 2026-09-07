import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  getInstallInfo,
  launchUninstaller,
  resetAllData,
} from "./filmLibrary";
import { clearAllSettings } from "./settings";
import { relaunchApp } from "./devReload";

export function installKindLabel(kind: string): string {
  switch (kind) {
    case "installed":
      return "Installed release";
    case "dev":
      return "Development build";
    case "portable":
      return "Portable build";
    default:
      return "Unknown build";
  }
}

export async function openDataFolder(): Promise<void> {
  const info = await getInstallInfo();
  await revealItemInDir(info.databasePath);
}

export async function openLogFile(): Promise<void> {
  const info = await getInstallInfo();
  await revealItemInDir(info.logPath);
}

export async function resetAppData(): Promise<void> {
  await resetAllData();
  await clearAllSettings();
  await relaunchApp();
}

export { getInstallInfo, launchUninstaller };
