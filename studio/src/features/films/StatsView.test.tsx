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

  it("places Most rewatched before Watching activity", async () => {
    renderStats();

    const rewatched = await screen.findByRole("heading", { name: /most rewatched/i });
    const activity = screen.getByRole("heading", { name: /watching activity/i });

    expect(
      rewatched.compareDocumentPosition(activity) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
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

  it("keeps activity and ratings as a single asymmetric row", async () => {
    renderStats();

    const activity = await screen.findByRole("heading", { name: /watching activity/i });
    const row = document.querySelector<HTMLElement>(".stats-viz-row");

    expect(row).not.toBeNull();
    expect(row!.querySelector(".stats-activity")).toContainElement(activity);
    expect(row!.querySelector(".stats-ratings")).not.toBeNull();
    expect(row!.querySelector(".stats-activity")!.compareDocumentPosition(
      row!.querySelector(".stats-ratings")!,
    ) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(document.querySelector(".stats-primary-row")).toBeNull();
  });

  it("renders genres and decades as ranked lists without a decade histogram", async () => {
    renderStats();

    const genresHeading = await screen.findByRole("heading", { name: /your genres/i });
    const row = document.querySelector<HTMLElement>(".stats-rank-row");
    const genres = document.querySelector<HTMLElement>(".stats-genres");
    const decades = document.querySelector<HTMLElement>(".stats-decades");

    expect(row).not.toBeNull();
    expect(row).toContainElement(genres);
    expect(row).toContainElement(decades);
    expect(genres).toContainElement(genresHeading);
    expect(genres!.querySelector(".stats-rank-list")).not.toBeNull();
    expect(genres!.querySelectorAll(".stats-genre-bar")).toHaveLength(snapshot.genres.length);
    expect(within(genres!).getByText("2 films")).toBeInTheDocument();
    expect(within(genres!).getByText("4.3 avg")).toBeInTheDocument();
    expect(decades!.querySelector(".stats-rank-list")).not.toBeNull();
    expect(decades!.querySelector(".stats-histogram")).toBeNull();
  });

  it("keeps Highest rated quieter and below the ranked lists", async () => {
    renderStats();

    const highestRated = await screen.findByRole("heading", { name: /highest rated/i });
    const shelf = highestRated.closest(".stats-shelf");
    const rankRow = document.querySelector(".stats-rank-row");

    expect(shelf).toHaveClass("stats-shelf-secondary");
    expect(rankRow!.compareDocumentPosition(shelf!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});
