import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  LibraryCoverage,
  LibraryItem,
  StatsSnapshot,
} from "../../platform/types/film";
import { StatsView } from "./StatsView";

const { getCoverage, getLibrary, getStats } = vi.hoisted(() => ({
  getCoverage: vi.fn(),
  getLibrary: vi.fn(),
  getStats: vi.fn(),
}));

vi.mock("../../platform/filmLibrary", () => ({
  getCoverage,
  getLibrary,
  getStats,
}));

const coverage: LibraryCoverage = {
  uniqueMovies: 2,
  watchlistMovies: 17,
  totalViewings: 5,
  ratingEvents: 2,
  unresolvedMovies: 0,
  source: "export",
  fullHistoryAvailable: true,
  warnings: [],
};

const films: LibraryItem[] = [
  {
    id: "film-1",
    title: "Repeat Viewing",
    year: 1999,
    currentRating: 5,
    poster: null,
    watched: true,
    watchlist: false,
    liked: true,
    viewingCount: 4,
    matchState: "matched",
    sourceType: "export",
    lastWatchedAt: "2026-08-01",
  },
  {
    id: "film-2",
    title: "One and Done",
    year: 2016,
    currentRating: 3.5,
    poster: null,
    watched: true,
    watchlist: false,
    liked: false,
    viewingCount: 1,
    matchState: "matched",
    sourceType: "export",
    lastWatchedAt: "2026-07-01",
  },
];

const snapshot: StatsSnapshot = {
  viewingMonths: [
    { label: "2026-07", count: 1, averageRating: null },
    { label: "2026-08", count: 4, averageRating: null },
  ],
  genres: [
    { label: "Drama", count: 2, averageRating: 4.25 },
    { label: "Comedy", count: 1, averageRating: 3.5 },
  ],
  rewatchCount: 3,
  totalRuntimeMinutes: 120,
  runtimeViewings: 1,
  metadataMovies: 2,
  people: [{ name: "Dee Director", role: "director", count: 4, averageRating: 4.5 }],
};

function renderStats() {
  return render(<StatsView onSelectFilm={vi.fn()} />);
}

describe("StatsView", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    getLibrary.mockResolvedValue({ items: films, total: films.length, coverage });
    getCoverage.mockResolvedValue(coverage);
    getStats.mockResolvedValue(snapshot);
  });

  it("does not render Genre Affinity", async () => {
    renderStats();

    await screen.findByRole("heading", { name: "Stats" });

    expect(screen.queryByText(/genre affinity/i)).not.toBeInTheDocument();
  });

  it("shows one stats view at a time", async () => {
    renderStats();

    expect(await screen.findByRole("heading", { name: /watching activity/i })).toBeInTheDocument();
    expect(document.querySelector(".stats-panel")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /^ratings$/i })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "Ratings" }));

    expect(screen.getByRole("heading", { name: /^ratings$/i })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /watching activity/i })).not.toBeInTheDocument();
    expect(document.querySelectorAll(".stats-panel .stats-section")).toHaveLength(1);
  });

  it("does not use a middot summary strip as the hero", async () => {
    renderStats();

    await screen.findByRole("heading", { name: "Stats" });

    expect(document.querySelector(".stats-summary")).toBeNull();
  });

  it("shows four viewing figures with a quiet all-time scope", async () => {
    renderStats();

    await screen.findByText("2h");
    const overview = document.querySelector<HTMLElement>(".stats-overview");

    expect(overview).not.toBeNull();
    expect(overview!.children).toHaveLength(4);
    expect(within(overview!).getByText("Films")).toBeInTheDocument();
    expect(within(overview!).getByText("Watches")).toBeInTheDocument();
    expect(within(overview!).getByText("Hours watched")).toBeInTheDocument();
    expect(within(overview!).getByText("Average rating")).toBeInTheDocument();
    expect(within(overview!).queryByText(/watchlist/i)).not.toBeInTheDocument();
    expect(screen.getByText("All time")).toHaveClass("stats-scope");
  });

  it("renders genres, decades, and people as the same ranked list", async () => {
    renderStats();

    fireEvent.click(await screen.findByRole("tab", { name: "Genres" }));
    const genres = document.querySelector<HTMLElement>(".stats-genres");
    expect(genres).not.toBeNull();
    expect(genres!.querySelectorAll(".stats-genre-bar")).toHaveLength(snapshot.genres.length);
    expect(within(genres!).getByText("2 films")).toBeInTheDocument();
    expect(within(genres!).getByText("4.3 avg")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /watching activity/i })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "Decades" }));
    const decades = document.querySelector<HTMLElement>(".stats-decades");
    expect(decades!.querySelector(".stats-rank-list")).not.toBeNull();
    expect(decades!.querySelector(".stats-histogram")).toBeNull();
    expect(within(decades!).getByText("1990s")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "People" }));
    expect(screen.getByText("Directors")).toBeInTheDocument();
    expect(screen.getByText("Dee Director")).toBeInTheDocument();
  });
});
