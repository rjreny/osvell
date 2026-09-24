import { cleanup, render, screen, within } from "@testing-library/react";
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

  it("shows the retrospective sections together", async () => {
    renderStats();

    expect(await screen.findByRole("heading", { name: /watching activity/i })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /^ratings$/i })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /^genres$/i })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /^decades$/i })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /^people$/i })).toBeInTheDocument();
    const rewatched = screen.getByRole("heading", { name: /most rewatched/i }).closest("section");
    expect(within(rewatched!).getByText("4 watches")).toBeInTheDocument();
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
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

    const genres = await screen.findByRole("heading", { name: /^genres$/i });
    const genresSection = genres.closest("section");
    expect(genresSection).not.toBeNull();
    expect(genresSection!.querySelectorAll(".stats-genre-bar")).toHaveLength(snapshot.genres.length);
    expect(within(genresSection!).getByText("2 films")).toBeInTheDocument();
    expect(within(genresSection!).getByText("4.3 avg")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /watching activity/i })).toBeInTheDocument();

    const decades = screen.getByRole("heading", { name: /^decades$/i }).closest("section");
    expect(decades!.querySelector(".stats-rank-list")).not.toBeNull();
    expect(decades!.querySelector(".stats-histogram")).toBeNull();
    expect(within(decades!).getByText("1990s")).toBeInTheDocument();

    const people = screen.getByRole("heading", { name: /^people$/i }).closest("section");
    expect(within(people!).getByText("Directors")).toBeInTheDocument();
    expect(within(people!).getByText("Dee Director")).toBeInTheDocument();
  });
});
