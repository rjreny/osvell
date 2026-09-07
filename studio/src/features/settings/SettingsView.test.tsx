import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Accent, Theme } from "../../core/types";
import type { InstallInfo, LibraryCoverage, TasteKeyStatus } from "../../platform/types/film";
import { SettingsView } from "./SettingsView";

const { checkAppUpdate, getInstallInfo, listen, tasteKeyStatus, tasteSetModel, tmdbKeyStatus } = vi.hoisted(() => ({
  checkAppUpdate: vi.fn(),
  getInstallInfo: vi.fn(),
  listen: vi.fn(),
  tasteKeyStatus: vi.fn(),
  tasteSetModel: vi.fn(),
  tmdbKeyStatus: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn() }));
vi.mock("../../platform/files", () => ({ pickExportZipPath: vi.fn() }));
vi.mock("../../platform/filmLibrary", () => ({
  formatLibrarySummary: vi.fn(() => ""),
  formatBytes: vi.fn(() => ""),
  formatEnrich: vi.fn(() => ""),
  formatImport: vi.fn(() => ""),
  formatRssSyncAt: vi.fn(() => "Never"),
  importExportZip: vi.fn(),
  importGetDiagnostics: vi.fn(() => Promise.resolve({ warnings: [] })),
  syncFeeds: vi.fn(),
  tmdbClearKey: vi.fn(),
  tmdbEnrich: vi.fn(),
  tmdbKeyStatus,
  tmdbSetKey: vi.fn(),
  tasteClearKey: vi.fn(),
  tasteKeyStatus,
  tasteSetKey: vi.fn(),
  tasteSetModel,
  tasteSetWeb: vi.fn(),
}));
vi.mock("../../platform/install", () => ({
  getInstallInfo,
  installKindLabel: vi.fn(() => ""),
  launchUninstaller: vi.fn(),
  openDataFolder: vi.fn(),
  openLogFile: vi.fn(),
  resetAppData: vi.fn(),
}));
vi.mock("../../platform/log", () => ({ log: vi.fn() }));
vi.mock("../../platform/updater", () => ({
  checkAppUpdate,
  downloadAndInstallUpdate: vi.fn(),
}));

const emptyCoverage: LibraryCoverage = {
  uniqueMovies: 0,
  watchlistMovies: 0,
  totalViewings: 0,
  ratingEvents: 0,
  unresolvedMovies: 0,
  source: "none",
  fullHistoryAvailable: false,
  warnings: [],
};

const sampleInstall: InstallInfo = {
  version: "0.14.1",
  installKind: "installed",
  appDataDir: "C:\\Users\\Ryan\\AppData\\Roaming\\com.rjreny.studio",
  databasePath: "C:\\Users\\Ryan\\AppData\\Roaming\\com.rjreny.studio\\studio.db",
  executablePath: "C:\\Users\\Ryan\\AppData\\Local\\Studio\\studio.exe",
  uninstallerPath: "C:\\Users\\Ryan\\AppData\\Local\\Studio\\uninstall.exe",
  logPath: "C:\\Users\\Ryan\\AppData\\Roaming\\com.rjreny.studio\\studio.log",
  dataBytes: 2048,
};

const statusWithFourModels: TasteKeyStatus = {
  stored: false,
  valid: null,
  lastError: null,
  model: "deepseek/deepseek-v4-pro-0813",
  web: false,
  models: [
    {
      id: "deepseek/deepseek-v4-pro-0813",
      label: "DeepSeek V4 Pro 0813",
      blurb: "Best overall reader for nuanced taste.",
      context: "1M",
      cost: "$0.30/M",
    },
    {
      id: "google/gemini-3.7-flash",
      label: "Gemini 3.7 Flash",
      blurb: "Fast fallback for a lighter read.",
      context: "1M",
      cost: "$0.10/M",
    },
    {
      id: "anthropic/claude-sonnet-4.5",
      label: "Claude Sonnet 4.5",
      blurb: "Measured reasoning with polished prose.",
      context: "200K",
      cost: "$3/M",
    },
    {
      id: "openai/gpt-5.2",
      label: "GPT-5.2",
      blurb: "Broad film knowledge and clear synthesis.",
      context: "400K",
      cost: "$2/M",
    },
  ],
};

function renderSettings({
  coverage = emptyCoverage,
  tasteStatus,
  username = "",
  lastRssSyncAt,
  onTheme = vi.fn(),
  onAccent = vi.fn(),
}: {
  coverage?: LibraryCoverage | null;
  tasteStatus?: TasteKeyStatus;
  username?: string;
  lastRssSyncAt?: string | null;
  onTheme?: (theme: Theme) => void;
  onAccent?: (accent: Accent) => void;
} = {}) {
  if (tasteStatus) tasteKeyStatus.mockResolvedValue(tasteStatus);
  return render(
    <SettingsView
      theme="system"
      accent="app"
      version="0.12.3"
      username={username}
      coverage={coverage}
      lastRssSyncAt={lastRssSyncAt}
      onTheme={onTheme}
      onAccent={onAccent}
      onUsername={vi.fn()}
      onStatus={vi.fn()}
      onRefresh={vi.fn(() => Promise.resolve())}
    />,
  );
}

describe("SettingsView", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    listen.mockResolvedValue(() => {});
    tmdbKeyStatus.mockResolvedValue({ stored: false, valid: null, kind: null, lastError: null });
    tasteKeyStatus.mockResolvedValue({
      stored: false,
      valid: null,
      lastError: null,
      model: "deepseek/deepseek-v4-pro-0813",
      web: false,
      models: [],
    });
    tasteSetModel.mockResolvedValue(statusWithFourModels);
    getInstallInfo.mockResolvedValue(null);
  });

  it("exposes the four named rail destinations and shows one panel at a time", () => {
    renderSettings({ coverage: emptyCoverage });

    const rail = screen.getByRole("navigation", { name: "Settings" });
    const destinations = within(rail).getAllByRole("button");
    expect(destinations).toHaveLength(4);
    expect(destinations.map((destination) => destination.getAttribute("aria-label"))).toEqual([
      "Library",
      "Appearance",
      "Taste",
      "System",
    ]);
    expect(screen.getByRole("heading", { name: "Library", level: 2 })).toBeInTheDocument();

    fireEvent.click(within(rail).getByRole("button", { name: "Appearance" }));

    expect(screen.getByRole("heading", { name: "Appearance", level: 2 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Library", level: 2 })).not.toBeInTheDocument();

    fireEvent.click(within(rail).getByRole("button", { name: "Taste" }));

    expect(screen.getByRole("heading", { name: "Taste", level: 2 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Appearance", level: 2 })).not.toBeInTheDocument();

    fireEvent.click(within(rail).getByRole("button", { name: "System" }));

    expect(screen.getByRole("heading", { name: "System", level: 2 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Taste", level: 2 })).not.toBeInTheDocument();
  });

  it("colocates This PC and Updates under System with a local divider", async () => {
    getInstallInfo.mockResolvedValue(sampleInstall);
    renderSettings({
      coverage: {
        ...emptyCoverage,
        uniqueMovies: 12,
        totalViewings: 14,
        source: "export",
        fullHistoryAvailable: true,
      },
    });

    const rail = screen.getByRole("navigation", { name: "Settings" });
    expect(within(rail).getAllByRole("button")).toHaveLength(4);
    fireEvent.click(within(rail).getByRole("button", { name: "System" }));

    const thisPcHeading = screen.getByRole("heading", { name: "This PC", level: 3 });
    const updatesHeading = screen.getByRole("heading", { name: "Updates", level: 3 });
    const thisPc = thisPcHeading.closest("section");
    const updates = updatesHeading.closest("section");

    expect(thisPc).not.toBeNull();
    expect(updates).not.toBeNull();
    expect(await within(thisPc!).findByText(sampleInstall.appDataDir)).toHaveClass("mono-path");
    expect(within(thisPc!).getByRole("button", { name: "Open folder" })).toBeInTheDocument();
    expect(updates).toHaveClass("settings-system-updates");
    expect(within(updates!).getByText("Version 0.12.3")).toBeInTheDocument();
    expect(within(updates!).getByRole("button", { name: "Check for updates" })).toBeInTheDocument();
  });

  it("badges System after an update check without adding a rail destination", async () => {
    checkAppUpdate.mockResolvedValue({
      available: true,
      version: "0.13.0",
      signingConfigured: true,
    });
    renderSettings();

    const rail = screen.getByRole("navigation", { name: "Settings" });
    fireEvent.click(within(rail).getByRole("button", { name: "System" }));
    fireEvent.click(screen.getByRole("button", { name: /check/i }));

    await waitFor(() => {
      expect(within(rail).getAllByRole("button")).toHaveLength(4);
      expect(within(rail).getByRole("button", { name: "System, update available" })).toHaveTextContent(
        "Update available",
      );
    });
    expect(screen.getByRole("button", { name: "Update to 0.13.0" })).toBeInTheDocument();
  });

  it("presents theme choices as labeled previews and applies a selection", () => {
    const onTheme = vi.fn();
    renderSettings({ onTheme });
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));

    const themeChoices = within(screen.getByRole("radiogroup", { name: "Theme" }));
    expect(themeChoices.getByRole("radio", { name: "System" })).toBeInTheDocument();
    fireEvent.click(themeChoices.getByRole("radio", { name: "Dark" }));
    expect(themeChoices.getByRole("radio", { name: "Light" })).toBeInTheDocument();
    expect(onTheme).toHaveBeenCalledWith("dark");
  });

  it("presents Osvell blue and System as quiet accent radios and applies a selection", () => {
    const onAccent = vi.fn();
    renderSettings({ onAccent });
    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));

    const accentChoices = within(screen.getByRole("radiogroup", { name: "Accent" }));
    expect(accentChoices.getByRole("radio", { name: "Osvell blue" })).toBeInTheDocument();
    fireEvent.click(accentChoices.getByRole("radio", { name: "System" }));
    expect(onAccent).toHaveBeenCalledWith("system");
  });

  it.each([
    ["username alone", "Not connected", "filmfan", emptyCoverage, null],
    ["RSS refresh evidence", "Connected", "filmfan", emptyCoverage, "2026-09-05T18:00:00Z"],
    [
      "imported coverage",
      "Connected",
      "filmfan",
      { ...emptyCoverage, source: "export" as const, fullHistoryAvailable: true },
      null,
    ],
    [
      "coverage without an identity",
      "Not connected",
      " ",
      { ...emptyCoverage, source: "rss" as const },
      null,
    ],
  ])("shows %s as %s", (_case, expectedStatus, username, coverage, lastRssSyncAt) => {
    renderSettings({ username, coverage, lastRssSyncAt });

    expect(screen.getByText(expectedStatus, { selector: ".settings-letterboxd-status" })).toBeInTheDocument();
  });

  it("makes Import history primary when the library is empty", () => {
    renderSettings({
      coverage: {
        ...emptyCoverage,
        uniqueMovies: 0,
        source: "none",
        fullHistoryAvailable: false,
      },
    });

    const importButton = screen.getByRole("button", { name: /import history/i });
    expect(importButton.className).toMatch(/primary/);
  });

  it("elevates Sync now after the library is established", () => {
    renderSettings({
      coverage: {
        ...emptyCoverage,
        uniqueMovies: 120,
        source: "export",
        fullHistoryAvailable: true,
      },
    });

    expect(screen.getByRole("button", { name: /sync now/i }).className).toMatch(/primary/);
    expect(screen.getByRole("button", { name: /match posters/i }).className).not.toMatch(/primary/);
  });

  it.each([
    ["a non-empty source", { source: "rss" as const }],
    ["full history", { fullHistoryAvailable: true }],
  ])("treats %s as an established library", (_label, coverageOverride) => {
    renderSettings({ coverage: { ...emptyCoverage, ...coverageOverride } });

    expect(screen.getByRole("button", { name: /sync now/i }).className).toMatch(/primary/);
  });

  it("treats a meaningful movie count alone as an established library", () => {
    renderSettings({
      coverage: {
        ...emptyCoverage,
        uniqueMovies: 1,
        source: "none",
        fullHistoryAvailable: false,
      },
    });

    expect(screen.getByRole("button", { name: /sync now/i }).className).toMatch(/primary/);
  });

  it("keeps sync explanation collapsed until disclosed", () => {
    renderSettings({ coverage: emptyCoverage });

    expect(screen.queryByText(/once an hour/i)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /how syncing works/i }));
    expect(screen.getByText(/once an hour/i)).toBeInTheDocument();
  });

  it("uses quiet human labels for configured TMDB", async () => {
    tmdbKeyStatus.mockResolvedValue({
      stored: true,
      valid: true,
      kind: "credential",
      lastError: null,
    });

    renderSettings({ coverage: emptyCoverage });

    expect(await screen.findByText("Configured")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Change" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove" })).toBeInTheDocument();
    expect(screen.queryByText(/Credential Manager/i)).not.toBeInTheDocument();
  });

  it("does not show the full model zoo on the default Taste panel", async () => {
    renderSettings({ tasteStatus: statusWithFourModels });
    fireEvent.click(screen.getByRole("button", { name: "Taste" }));

    expect(await screen.findByText("DeepSeek V4 Pro 0813")).toBeInTheDocument();
    expect(screen.queryByText(/1M ·/i)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Change recommendation model" })).toBeInTheDocument();
  });

  it("opens a centered dialog for model selection", async () => {
    renderSettings({ tasteStatus: statusWithFourModels });
    fireEvent.click(screen.getByRole("button", { name: "Taste" }));
    fireEvent.click(await screen.findByRole("button", { name: "Change recommendation model" }));

    const dialog = await screen.findByRole("dialog", { name: /recommendation model/i });
    expect(dialog).toBeInTheDocument();
    expect(dialog.className).toMatch(/taste-model-modal/);
    expect(within(dialog).getAllByText(/1M/i)).toHaveLength(2);
  });

  it("applies a model choice and closes the dialog", async () => {
    renderSettings({ tasteStatus: statusWithFourModels });
    fireEvent.click(screen.getByRole("button", { name: "Taste" }));
    fireEvent.click(await screen.findByRole("button", { name: "Change recommendation model" }));
    const dialog = await screen.findByRole("dialog", { name: /recommendation model/i });

    fireEvent.click(within(dialog).getByRole("button", { name: /Gemini 3.7 Flash/i }));

    expect(screen.queryByRole("dialog", { name: /recommendation model/i })).not.toBeInTheDocument();
    await waitFor(() => expect(tasteSetModel).toHaveBeenCalledWith("google/gemini-3.7-flash"));
  });

  it("dismisses the model dialog with Escape and restores focus to Change", async () => {
    renderSettings({ tasteStatus: statusWithFourModels });
    fireEvent.click(screen.getByRole("button", { name: "Taste" }));
    const change = await screen.findByRole("button", { name: "Change recommendation model" });
    fireEvent.click(change);
    await screen.findByRole("dialog", { name: /recommendation model/i });

    fireEvent.keyDown(document, { key: "Escape" });

    expect(screen.queryByRole("dialog", { name: /recommendation model/i })).not.toBeInTheDocument();
    expect(change).toHaveFocus();
  });

  it("dismisses the model dialog from its backdrop", async () => {
    renderSettings({ tasteStatus: statusWithFourModels });
    fireEvent.click(screen.getByRole("button", { name: "Taste" }));
    const change = await screen.findByRole("button", { name: "Change recommendation model" });
    fireEvent.click(change);
    const dialog = await screen.findByRole("dialog", { name: /recommendation model/i });

    fireEvent.click(dialog);

    expect(screen.queryByRole("dialog", { name: /recommendation model/i })).not.toBeInTheDocument();
    expect(change).toHaveFocus();
  });
});
