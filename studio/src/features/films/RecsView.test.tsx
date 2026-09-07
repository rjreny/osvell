import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TastePick, TasteState } from "../../platform/types/film";
import { RecsView } from "./RecsView";

const { listen, tasteAnalyze, tasteFeedbackSet, tasteGet } = vi.hoisted(() => ({
  listen: vi.fn(),
  tasteAnalyze: vi.fn(),
  tasteFeedbackSet: vi.fn(),
  tasteGet: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("../../platform/filmLibrary", () => ({ tasteAnalyze, tasteFeedbackSet, tasteGet }));

const feature = {
  featureKey: "mood:neo-noir",
  name: "neo-noir",
  family: "mood",
  appearances: 6,
  recommendationMean: 0.8,
  scoringAffinity: 0.8,
  confidence: 0.8,
  portability: 0.8,
  citeable: true,
  cited: true,
};

function pick(title: string, tmdbId: number): TastePick {
  return {
    title,
    year: 2024,
    poster: null,
    why: "A good fit.",
    rhymesWith: [],
    filmId: `tmdb:${tmdbId}`,
    tmdbId,
    source: "tmdb",
    matchScore: 74,
    attribution: {
      exposureId: `exposure-${tmdbId}`,
      runId: "run-1",
      tmdbId,
      title,
      evidenceGrade: "strong",
      citedPositive: [feature],
      citedNegative: [],
      seedFilms: [],
      semanticFit: 0.8,
      diversityAdjustment: 0,
      retrievalSource: "tmdb",
      rankingRationale: [],
      moodSignature: { modes: ["noir", "tense"], thematicKeywords: [] },
      priorCandidateExposures: 0,
      priorFeatureExposures: [],
    },
  };
}

function state(overrides: Partial<TasteState> = {}): TasteState {
  return {
    key: { stored: true, valid: true, lastError: null, model: "model", web: false, models: [] },
    snapshot: { ratedCount: 10, lovedCount: 5, hatedCount: 1, avgRating: 4, genres: [], decades: [], directors: [] },
    feedback: [],
    report: {
      title: "Taste",
      summary: "",
      affinities: [],
      aversions: [],
      dimensions: [],
      newPicks: [pick("New recommendation", 101)],
      watchlistPicks: [pick("Watchlist recommendation", 202)],
      picks: [],
      model: "model",
      generatedAt: "2026-08-28T00:00:00Z",
      ratedCount: 10,
    },
    ...overrides,
  };
}

describe("RecsView feedback", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    listen.mockResolvedValue(() => {});
    tasteGet.mockResolvedValue(state());
    tasteFeedbackSet.mockResolvedValue({});
  });

  it("sends the selected Pass reason and target feature to Taste", async () => {
    render(<RecsView onSelectFilm={vi.fn()} onOpenSettings={vi.fn()} />);

    const title = await screen.findByRole("button", { name: "New recommendation" });
    const card = title.closest("article");
    expect(card).not.toBeNull();
    fireEvent.click(within(card!).getByRole("radio", { name: "Pass" }));
    fireEvent.click(await screen.findByRole("button", { name: "That connection doesn't fit" }));

    await waitFor(() => {
      expect(tasteFeedbackSet).toHaveBeenCalledWith(101, "rejected", {
        exposureId: "exposure-101",
        reason: "wrong_connection",
        targetFeatureKey: "mood:neo-noir",
      });
    });
  });

  it("labels positive feedback as interest instead of a duplicate save", async () => {
    render(<RecsView onSelectFilm={vi.fn()} onOpenSettings={vi.fn()} />);

    const newCard = (await screen.findByRole("button", { name: "New recommendation" })).closest("article");
    const watchlistCard = screen.getByRole("button", { name: "Watchlist recommendation" }).closest("article");
    expect(within(newCard!).getByRole("radio", { name: "Interested" })).toBeInTheDocument();
    expect(within(watchlistCard!).getByRole("radio", { name: "Interested" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  });

  it("opens match evidence in a portal popover and closes it with Escape", async () => {
    render(<RecsView onSelectFilm={vi.fn()} onOpenSettings={vi.fn()} />);

    const card = (await screen.findByRole("button", { name: "New recommendation" })).closest("article");
    const trigger = within(card!).getByRole("button", { name: "Why New recommendation is a 74% match" });
    fireEvent.click(trigger);

    const dialog = await screen.findByRole("dialog", { name: "Why this 74% match" });
    expect(card).not.toContainElement(dialog);
    expect(dialog).toHaveTextContent("Strong evidence");
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Why this 74% match" })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("keeps a stale board visible with a refresh prompt", async () => {
    tasteGet.mockResolvedValue(state({ reportStale: true }));
    render(<RecsView onSelectFilm={vi.fn()} onOpenSettings={vi.fn()} />);

    expect(await screen.findByText("Saved from an earlier Taste version")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New recommendation" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Read again" }).length).toBeGreaterThan(0);
  });

  it("centers the empty Taste stage before a run", async () => {
    tasteGet.mockResolvedValue(
      state({
        report: null,
        snapshot: {
          ratedCount: 12,
          lovedCount: 4,
          hatedCount: 2,
          avgRating: 3.5,
          genres: [{ label: "Thriller", count: 5, avg: 4 }],
          decades: [],
          directors: [{ label: "Mann", count: 2, avg: 5 }],
        },
      }),
    );
    render(<RecsView onSelectFilm={vi.fn()} onOpenSettings={vi.fn()} />);

    expect(await screen.findByRole("heading", { name: "Find your next film" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Read my log" })).toBeInTheDocument();
    expect(screen.getByText("Thriller")).toBeInTheDocument();
  });
});
