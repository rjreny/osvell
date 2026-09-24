import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HomeViewModel } from "../../platform/types/film";
import { HomeView } from "./HomeView";

const { getFilm, tasteGet } = vi.hoisted(() => ({
  getFilm: vi.fn(),
  tasteGet: vi.fn(),
}));

vi.mock("../../platform/filmLibrary", () => ({ getFilm, tasteGet }));

const home: HomeViewModel = {
  coverage: {
    uniqueMovies: 2,
    watchlistMovies: 0,
    totalViewings: 2,
    ratingEvents: 1,
    unresolvedMovies: 0,
    source: "export",
    fullHistoryAvailable: true,
    warnings: [],
  },
  recent: [],
  topRated: [],
  friendFeed: [],
  series: {
    name: "Before",
    watched: 1,
    total: 2,
    parts: [
      {
        id: "tmdb:1",
        title: "Before Sunrise",
        year: 1995,
        poster: null,
        watched: true,
        currentRating: 5,
        openable: true,
      },
      {
        id: "tmdb:2",
        title: "Before Sunset",
        year: 2004,
        poster: null,
        watched: false,
        currentRating: null,
        openable: true,
      },
    ],
  },
};

function renderHome() {
  return render(
    <HomeView home={home} onOpenFilms={vi.fn()} onOpenFriends={vi.fn()} onSelectFilm={vi.fn()} />,
  );
}

describe("HomeView watch shelf", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    getFilm.mockResolvedValue(null);
  });

  it("selects Watch Next once recommendations arrive alongside a series", async () => {
    let resolveTaste: (value: unknown) => void = () => {};
    tasteGet.mockReturnValue(new Promise((resolve) => {
      resolveTaste = resolve;
    }));

    renderHome();
    expect(screen.queryByRole("tab", { name: "Finish Before" })).not.toBeInTheDocument();

    resolveTaste({
      report: {
        picks: [{ title: "Heat", year: 1995, poster: null, filmId: "tmdb:949", tmdbId: 949 }],
      },
    });

    const watchNext = await screen.findByRole("tab", { name: "Watch Next" });
    expect(watchNext).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Finish Before" })).toHaveAttribute("aria-selected", "false");
    expect(screen.getByRole("button", { name: /Heat/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Before Sunrise/ })).not.toBeInTheDocument();
  });

  it("keeps an explicit Finish choice, then returns to Watch Next", async () => {
    tasteGet.mockResolvedValue({
      report: {
        picks: [{ title: "Heat", year: 1995, poster: null, filmId: "tmdb:949", tmdbId: 949 }],
      },
    });
    renderHome();

    fireEvent.click(await screen.findByRole("tab", { name: "Finish Before" }));
    expect(screen.getByRole("tab", { name: "Finish Before" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("button", { name: /Before Sunrise/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Heat/ })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "Watch Next" }));
    expect(screen.getByRole("tab", { name: "Watch Next" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("button", { name: /Heat/ })).toBeInTheDocument();
  });

  it("falls back to the series when there is nothing to watch next", async () => {
    tasteGet.mockResolvedValue({ report: { picks: [] } });
    renderHome();

    const finish = await screen.findByRole("tab", { name: "Finish Before" });
    expect(finish).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("tab", { name: "Watch Next" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Before Sunrise/ })).toBeInTheDocument();
  });
});
