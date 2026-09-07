import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FilmDetail } from "../../platform/types/film";
import { FilmDetailView } from "./FilmDetailView";

const { getFilm, getFilmArtwork, setFilmArtwork } = vi.hoisted(() => ({
  getFilm: vi.fn(),
  getFilmArtwork: vi.fn(),
  setFilmArtwork: vi.fn(),
}));

vi.mock("../../platform/filmLibrary", () => ({ getFilm, getFilmArtwork, setFilmArtwork }));
vi.mock("../../platform/log", () => ({ log: vi.fn() }));

const cast = Array.from({ length: 18 }, (_, index) => ({
  tmdbId: index + 1,
  name: `Cast member ${index + 1}`,
  profile: null,
  character: index ? null : "Lead",
  order: index,
}));

const film: FilmDetail = {
  id: "tmdb:1",
  title: "Example film",
  year: 2024,
  currentRating: 4,
  poster: null,
  backdrop: null,
  overview: "A precise test of the detail presentation.",
  runtime: 110,
  genres: ["Drama"],
  matchState: "confirmed",
  sourceIdentity: "letterboxd_export",
  yourHistory: [],
  friends: [],
  tmdbId: 1,
  tmdbVoteAverage: 8.4,
  tmdbVoteCount: 1200,
  tmdbReviews: [],
  tagline: null,
  directors: ["A Director"],
  cast,
  crew: [],
  companies: [],
  keywords: [],
  connections: [],
  collectionName: null,
  collection: [],
  similar: [],
  trailers: [],
};

function renderDetail(onBack = vi.fn()) {
  return render(
    <FilmDetailView
      filmId={film.id}
      onBack={onBack}
      backLabel="Back to Home"
      onStatus={vi.fn()}
    />,
  );
}

describe("FilmDetailView", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    getFilm.mockResolvedValue(film);
  });

  it("keeps a truthful page-local return action and only the community rating in the hero", async () => {
    const onBack = vi.fn();
    renderDetail(onBack);

    const back = await screen.findByRole("button", { name: "Back to Home" });
    expect(screen.getByText("TMDB")).toBeInTheDocument();
    expect(screen.queryByText("Your rating")).not.toBeInTheDocument();
    expect(screen.queryByText("Taste fit")).not.toBeInTheDocument();
    fireEvent.click(back);
    expect(onBack).toHaveBeenCalledOnce();
  });

  it("makes every returned cast credit reachable", async () => {
    renderDetail();
    const expand = await screen.findByRole("button", { name: "Show all 18 cast members" });
    expect(screen.getByText("Lead")).toBeInTheDocument();
    expect(screen.queryByText("Cast member 18")).not.toBeInTheDocument();
    fireEvent.click(expand);
    expect(screen.getByText("Cast member 18")).toBeInTheDocument();
  });

  it("keeps only the latest viewing beside the overview", async () => {
    getFilm.mockResolvedValue({
      ...film,
      yourHistory: [
        { id: "latest", occurredAt: "2026-08-31", publishedAt: null, rewatch: false, rating: 3, source: "letterboxd_rss" },
        { id: "older", occurredAt: "2026-08-01", publishedAt: null, rewatch: false, rating: 4, source: "local" },
      ],
    });
    renderDetail();

    expect(await screen.findByRole("heading", { name: "Last rating" })).toBeInTheDocument();
    expect(screen.getByText("Letterboxd RSS")).toBeInTheDocument();
    expect(screen.queryByText("Osvell")).not.toBeInTheDocument();
  });

  it("plays trailers inside the detail view without leaving the app", async () => {
    getFilm.mockResolvedValue({
      ...film,
      trailers: [
        { key: "abc123", name: "Official Trailer", site: "YouTube", type: "Trailer", official: true },
        { key: "def456", name: "Teaser", site: "YouTube", type: "Teaser", official: false },
      ],
    });
    renderDetail();

    fireEvent.click(await screen.findByRole("button", { name: "Play trailer (2)" }));
    expect(screen.getByRole("dialog", { name: "Official Trailer trailer" })).toBeInTheDocument();
    const frame = screen.getByTitle("Official Trailer");
    expect(frame).toHaveAttribute("src", expect.stringContaining("youtube-nocookie.com/embed/abc123"));
    fireEvent.click(screen.getByRole("tab", { name: "Teaser" }));
    expect(screen.getByTitle("Teaser")).toHaveAttribute(
      "src",
      expect.stringContaining("youtube-nocookie.com/embed/def456"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("offers TMDB artwork choices and saves the selected poster", async () => {
    getFilmArtwork.mockResolvedValue({
      posters: [{ path: "/alternate-poster.jpg", url: "https://image.tmdb.org/t/p/w780/alternate-poster.jpg" }],
      backdrops: [{ path: "/alternate-backdrop.jpg", url: "https://image.tmdb.org/t/p/original/alternate-backdrop.jpg" }],
      selectedPoster: "/default-poster.jpg",
      selectedBackdrop: "/default-backdrop.jpg",
      defaultPoster: "/default-poster.jpg",
      defaultBackdrop: "/default-backdrop.jpg",
    });
    setFilmArtwork.mockResolvedValue({ ...film, poster: "https://image.tmdb.org/t/p/w780/alternate-poster.jpg" });
    renderDetail();

    fireEvent.click(await screen.findByRole("button", { name: "Customize artwork" }));
    fireEvent.click(await screen.findByRole("button", { name: "Use poster 1" }));

    await waitFor(() => expect(setFilmArtwork).toHaveBeenCalledWith(film.id, {
      poster: "/alternate-poster.jpg",
      backdrop: "/default-backdrop.jpg",
    }));
  });

  it("renders artwork choices progressively instead of loading the whole gallery", async () => {
    getFilmArtwork.mockResolvedValue({
      posters: Array.from({ length: 30 }, (_, index) => ({ path: `/poster-${index}.jpg`, url: `https://image.tmdb.org/t/p/w780/poster-${index}.jpg` })),
      backdrops: Array.from({ length: 14 }, (_, index) => ({ path: `/backdrop-${index}.jpg`, url: `https://image.tmdb.org/t/p/original/backdrop-${index}.jpg` })),
      selectedPoster: "/default-poster.jpg",
      selectedBackdrop: "/default-backdrop.jpg",
      defaultPoster: "/default-poster.jpg",
      defaultBackdrop: "/default-backdrop.jpg",
    });
    renderDetail();

    fireEvent.click(await screen.findByRole("button", { name: "Customize artwork" }));
    expect(await screen.findAllByRole("button", { name: /Use poster/ })).toHaveLength(24);
    expect(screen.getAllByRole("button", { name: /Use hero background/ })).toHaveLength(12);

    fireEvent.click(screen.getByRole("button", { name: "Show 6 more posters" }));
    fireEvent.click(screen.getByRole("button", { name: "Show 2 more backgrounds" }));
    expect(screen.getAllByRole("button", { name: /Use poster/ })).toHaveLength(30);
    expect(screen.getAllByRole("button", { name: /Use hero background/ })).toHaveLength(14);
  });

});
