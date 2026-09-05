import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LibraryCoverage } from "../../platform/types/film";
import { SettingsView } from "./SettingsView";

const { getInstallInfo, listen, tasteKeyStatus, tmdbKeyStatus } = vi.hoisted(() => ({
  getInstallInfo: vi.fn(),
  listen: vi.fn(),
  tasteKeyStatus: vi.fn(),
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
  tasteSetModel: vi.fn(),
  tasteSetWeb: vi.fn(),
}));
vi.mock("../../platform/install", () => ({
  getInstallInfo,
  installKindLabel: vi.fn(() => ""),
  launchUninstaller: vi.fn(),
  openDataFolder: vi.fn(),
  openLogFile: vi.fn(),
  resetStudioData: vi.fn(),
}));
vi.mock("../../platform/log", () => ({ log: vi.fn() }));
vi.mock("../../platform/updater", () => ({
  checkAppUpdate: vi.fn(),
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

function renderSettings({ coverage = emptyCoverage }: { coverage?: LibraryCoverage | null } = {}) {
  return render(
    <SettingsView
      theme="system"
      accent="app"
      version="0.12.3"
      username=""
      coverage={coverage}
      onTheme={vi.fn()}
      onAccent={vi.fn()}
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
    getInstallInfo.mockResolvedValue(null);
  });

  it("exposes exactly four rail destinations and shows one panel at a time", () => {
    renderSettings({ coverage: emptyCoverage });

    expect(screen.getByRole("navigation", { name: /settings/i })).toBeInTheDocument();
    expect(
      screen.getAllByRole("button", { name: /^(Library|Appearance|Taste|System)$/ }),
    ).toHaveLength(4);
    expect(screen.getByRole("heading", { name: "Library", level: 2 })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));

    expect(screen.getByRole("heading", { name: "Appearance", level: 2 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Library", level: 2 })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Taste" }));

    expect(screen.getByRole("heading", { name: "Taste", level: 2 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Appearance", level: 2 })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "System" }));

    expect(screen.getByRole("heading", { name: "System", level: 2 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Taste", level: 2 })).not.toBeInTheDocument();
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
});
