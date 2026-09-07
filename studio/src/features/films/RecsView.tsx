import { useEffect, useId, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { tasteAnalyze, tasteFeedbackSet, tasteGet } from "../../platform/filmLibrary";
import type { JobProgress, TasteFeedback, TasteModelInfo, TastePick, TasteState } from "../../platform/types/film";
import { log } from "../../platform/log";
import { Menu } from "../ui/Menu";
import { AnchoredPopover } from "../ui/AnchoredPopover";
import { Poster } from "./Poster";

const RUN_STEPS = [
  "Reading your log",
  "Scoring candidates",
  "Critiquing the shortlist",
  "Targeted discovery",
  "Taste profile",
  "Matching posters",
];

function formatElapsed(seconds: number) {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return m > 0 ? `${m}:${String(s).padStart(2, "0")}` : `${s}s`;
}

function pickId(pick: TastePick) {
  return pick.filmId || (pick.tmdbId ? `tmdb:${pick.tmdbId}` : null);
}

function pickKey(pick: TastePick) {
  if (pick.tmdbId) return `tmdb:${pick.tmdbId}`;
  if (pick.filmId) return pick.filmId;
  return `${pick.title}-${pick.year ?? ""}`;
}

function sectionPicks(report: NonNullable<TasteState["report"]>) {
  // Unified board: prefer `picks`, fall back to merging legacy shelf arrays once.
  if (Array.isArray(report.picks) && report.picks.length) {
    return report.picks;
  }
  if (Array.isArray(report.newPicks) || Array.isArray(report.watchlistPicks)) {
    return [
      ...(report.newPicks ?? []),
      ...(report.explorePicks ?? []),
      ...(report.watchlistPicks ?? []),
    ];
  }
  return [] as TastePick[];
}

function waitHint() {
  return "Usually under a minute. If OpenRouter blocks a model, Taste retries without web search, then with Gemini 3.7 Flash.";
}

function likedIds(feedback: TasteFeedback[] | undefined) {
  return new Set(
    (feedback ?? []).filter((row) => row.action === "interested").map((row) => row.tmdbId),
  );
}

type TasteSort = "recommended" | "match" | "title" | "year";
type TasteFilter = "all" | "50" | "60" | "70";
type WatchlistFilter = "all" | "not_watchlist" | "watchlist";
type TasteFeedbackOptions = {
  reason?: "already_seen_disliked" | "not_this_kind" | "wrong_connection" | "not_in_the_mood";
  targetFeatureKey?: string;
  moodScope?: "this_movie_only" | "this_kind_right_now";
};

type TastePopoverKind = "why" | "pass";
type OpenTastePopover = {
  key: string;
  kind: TastePopoverKind;
  trigger: HTMLButtonElement;
  openedWithKeyboard: boolean;
};

const TASTE_SORTS: { id: TasteSort; label: string }[] = [
  { id: "recommended", label: "Recommended" },
  { id: "match", label: "Highest match" },
  { id: "title", label: "Title A–Z" },
  { id: "year", label: "Newest" },
];

const TASTE_FILTERS: { id: TasteFilter; label: string }[] = [
  { id: "all", label: "Any score" },
  { id: "70", label: "70%+" },
  { id: "60", label: "60%+" },
  { id: "50", label: "50%+" },
];

const WATCHLIST_FILTERS: { id: WatchlistFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "not_watchlist", label: "Not on watchlist" },
  { id: "watchlist", label: "On watchlist" },
];

function matchValue(pick: TastePick) {
  return typeof pick.matchScore === "number" && Number.isFinite(pick.matchScore)
    ? pick.matchScore
    : -1;
}

function matchPercent(pick: TastePick) {
  const score = matchValue(pick);
  return score >= 0 ? `${Math.round(score)}% match` : "—% match";
}

function matchTone(pick: TastePick) {
  const score = matchValue(pick);
  if (score >= 70) return "is-high";
  if (score >= 60) return "is-mid";
  return score >= 0 ? "is-low" : "is-unknown";
}

/** Filter hides rows without recomputing underlying recommendation order. */
function filterTastePicks(
  picks: TastePick[],
  sort: TasteSort,
  matchFilter: TasteFilter,
  watchlistFilter: WatchlistFilter,
) {
  const filtered = picks.filter((pick) => {
    if (matchFilter !== "all" && matchValue(pick) < Number(matchFilter)) return false;
    if (watchlistFilter === "watchlist" && !pick.watchlist) return false;
    if (watchlistFilter === "not_watchlist" && pick.watchlist) return false;
    return true;
  });
  if (sort === "recommended") return filtered;
  return [...filtered].sort((a, b) => {
    if (sort === "title") return a.title.localeCompare(b.title);
    if (sort === "year") {
      return (b.year ?? -Infinity) - (a.year ?? -Infinity);
    }
    return matchValue(b) - matchValue(a);
  });
}

function listCount(shown: number, total: number) {
  return shown === total ? String(shown) : `${shown} of ${total}`;
}

function sentenceCase(value: string) {
  return value ? `${value[0].toLocaleUpperCase()}${value.slice(1)}` : value;
}

export function RecsView({
  onSelectFilm,
  onOpenSettings,
}: {
  onSelectFilm: (id: string) => void;
  onOpenSettings: () => void;
}) {
  const [state, setState] = useState<TasteState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [job, setJob] = useState<JobProgress | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [interested, setInterested] = useState<Set<number>>(new Set());

  useEffect(() => {
    let cancelled = false;
    void tasteGet()
      .then((next) => {
        if (!cancelled) {
          setState(next);
          setInterested(likedIds(next.feedback));
        }
      })
      .catch((err) => {
        log("warn", "taste load failed", err);
        if (!cancelled) setError("Could not read your log for Taste.");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<JobProgress>("studio-job", (event) => {
      const next = event.payload;
      if (next.job !== "taste") return;
      if (!next.done) {
        setRunning(true);
        setJob(next);
        return;
      }
      setRunning(false);
      setJob(null);
      if (next.errors) {
        setError(next.label.replace(/^taste failed · /i, "") || "Taste read failed");
        return;
      }
      void tasteGet()
        .then((loaded) => {
          setState(loaded);
          setInterested(likedIds(loaded.feedback));
          setHidden(new Set());
          setError(null);
        })
        .catch((err) => log("warn", "taste refresh failed", err));
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    if (!running) {
      setElapsed(0);
      return;
    }
    const started = Date.now();
    const id = window.setInterval(() => {
      setElapsed(Math.floor((Date.now() - started) / 1000));
    }, 1000);
    return () => window.clearInterval(id);
  }, [running]);

  async function run(forceRefresh = false) {
    try {
      setError(null);
      setRunning(true);
      setHidden(new Set());
      setJob({
        job: "taste",
        label: forceRefresh ? "Refreshing metadata…" : "Reading your log…",
        current: 1,
        total: 3,
        posters: 0,
        errors: 0,
        done: false,
      });
      await tasteAnalyze(forceRefresh);
    } catch (err) {
      setRunning(false);
      setJob(null);
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      log("error", "taste analyze failed", err);
    }
  }

  async function sendFeedback(
    pick: TastePick,
    action: "interested" | "rejected" | "seen",
    options: TasteFeedbackOptions = {},
  ) {
    const id = pick.tmdbId;
    if (!id) {
      setError("This title is missing a TMDB id, so feedback cannot be saved.");
      return;
    }
    if (!pick.attribution?.exposureId) {
      setError("This recommendation was created before attribution was available. Refresh Taste, then try again.");
      return;
    }
    const key = pickKey(pick);
    const prevHidden = new Set(hidden);
    const prevLiked = new Set(interested);
    if (action === "interested") {
      setInterested(new Set(prevLiked).add(id));
    } else {
      setHidden(new Set(prevHidden).add(key));
    }
    try {
      await tasteFeedbackSet(id, action, {
        ...options,
        exposureId: pick.attribution.exposureId,
      });
      const next = await tasteGet();
      setState(next);
      setInterested(likedIds(next.feedback));
      setHidden(new Set());
    } catch (err) {
      setHidden(prevHidden);
      setInterested(prevLiked);
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
    }
  }

  const snapshot = state?.snapshot;
  const report = state?.report;
  const key = state?.key;
  const keyReady = Boolean(key?.stored && key.valid !== false);
  const enoughRatings = (snapshot?.ratedCount ?? 0) >= 8;
  const step = job?.current ?? 1;

  return (
    <div className="recs page-pad">
      <header className="page-head">
        <div>
          <h1>{report?.title ?? "Taste"}</h1>
          <p className="muted">
            {snapshot
              ? `${report?.ratedCount ?? snapshot.ratedCount} ratings · ${snapshot.lovedCount} loved · ${snapshot.hatedCount} disliked`
              : "Find new films from your imported history."}
          </p>
        </div>
        {keyReady && enoughRatings ? (
          <div className="taste-actions">
            <button type="button" className="primary" disabled={running} onClick={() => void run()}>
              {running ? "Reading…" : report ? "Read again" : "Read my log"}
            </button>
            {report ? (
              <button
                type="button"
                className="taste-refresh"
                aria-label="Refresh recommendation metadata"
                title="Refresh recommendation metadata"
                disabled={running}
                onClick={() => void run(true)}
              >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M20 11a8 8 0 1 0 2 5.2" />
                  <path d="M20 4v7h-7" />
                </svg>
              </button>
            ) : null}
          </div>
        ) : null}
      </header>

      {error ? <p className="taste-error">{error}</p> : null}

      {!keyReady && state ? (
        <section className="taste-setup">
          <h2>Pay as you go</h2>
          <p>
            Taste uses OpenRouter. DeepSeek V4 Pro 0813 is the recommended default from models
            your key can actually reach. Add a few dollars of credit, paste the key in Settings,
            then come back.
          </p>
          <button type="button" className="primary" onClick={onOpenSettings}>
            Open Settings
          </button>
        </section>
      ) : null}

      {keyReady && !enoughRatings && snapshot ? (
        <p className="muted pad">Rate at least 8 films so the agent has likes and dislikes to compare.</p>
      ) : null}

      {running ? (
        <section className="taste-run" aria-live="polite">
          <div className="taste-run-head">
            <strong>{job?.label ?? "Reading your log…"}</strong>
            <span className="muted taste-elapsed">{formatElapsed(elapsed)}</span>
          </div>
          <ol>
            {RUN_STEPS.map((label, index) => {
              const n = index + 1;
              const cls = n < step ? "is-done" : n === step ? "is-now" : "";
              return (
                <li key={label} className={cls}>
                  <span>{n}</span>
                  {label}
                </li>
              );
            })}
          </ol>
          <p className="muted">{waitHint()}</p>
        </section>
      ) : null}

      {report ? (
        <TasteLists
          report={report}
          hidden={hidden}
          interested={interested}
          onSelectFilm={onSelectFilm}
          onFeedback={(pick, action, options) => void sendFeedback(pick, action, options)}
        />
      ) : null}

      {keyReady && enoughRatings && !report && !running ? (
        <p className="muted pad">
          The scorer reads every rating, then ranks up to 50 films quality-first.
          Match % is Content Fit (how well a title fits your taste), not the ranking key.
          Watchlist titles stay in that same order with a badge.
        </p>
      ) : null}

      {report ? <TasteProfileFooter report={report} observation={state?.observation} /> : null}
    </div>
  );
}

function TasteLists({
  report,
  hidden,
  interested,
  onSelectFilm,
  onFeedback,
}: {
  report: NonNullable<TasteState["report"]>;
  hidden: Set<string>;
  interested: Set<number>;
  onSelectFilm: (id: string) => void;
  onFeedback: (pick: TastePick, action: "interested" | "rejected" | "seen", options?: TasteFeedbackOptions) => void;
}) {
  const all = sectionPicks(report);
  const visible = all.filter((p) => !hidden.has(pickKey(p)));
  const [sort, setSort] = useState<TasteSort>("recommended");
  const [filter, setFilter] = useState<TasteFilter>("all");
  const [watchlistFilter, setWatchlistFilter] = useState<WatchlistFilter>("all");
  const [openPopover, setOpenPopover] = useState<OpenTastePopover | null>(null);
  const sorted = filterTastePicks(visible, sort, filter, watchlistFilter);

  return (
    <section>
      <div className="shelf-head taste-shelf-head">
        <div className="taste-list-heading">
          <h2>Recommendations</h2>
          <span className="muted">{listCount(sorted.length, visible.length)} picks</span>
        </div>
        {all.length ? (
          <div className="flat-menu-toolbar taste-list-toolbar" role="group" aria-label="Recommendation list controls">
            <Menu label="Sort" value={sort} options={TASTE_SORTS} onChange={(id) => setSort(id)} />
            <Menu
              label="Watchlist"
              value={watchlistFilter}
              options={WATCHLIST_FILTERS}
              onChange={(id) => setWatchlistFilter(id)}
            />
            <Menu label="Match" value={filter} options={TASTE_FILTERS} onChange={(id) => setFilter(id)} />
          </div>
        ) : null}
      </div>
      {sorted.length ? (
        <ul className="rec-list taste-rec-list">
          {sorted.map((pick) => (
            <TastePickCard
              key={pickKey(pick)}
              pick={pick}
              interested={Boolean(pick.tmdbId && interested.has(pick.tmdbId))}
              onSelectFilm={onSelectFilm}
              onFeedback={onFeedback}
              openPopover={openPopover?.key === pickKey(pick) ? openPopover : null}
              onOpenPopover={(kind, trigger, openedWithKeyboard) =>
                setOpenPopover({ key: pickKey(pick), kind, trigger, openedWithKeyboard })
              }
              onClosePopover={() => setOpenPopover(null)}
            />
          ))}
        </ul>
      ) : (
        <p className="muted pad">
          {visible.length ? "No films match this filter." : "Nothing cleared this run."}
        </p>
      )}
    </section>
  );
}

function TasteProfileFooter({
  report,
  observation,
}: {
  report: NonNullable<TasteState["report"]>;
  observation?: TasteState["observation"];
}) {
  const summary = report.summary || report.note;
  const affinities = report.affinities.slice(0, 2).map((item) => item.label);
  const exceptions = report.diagnostics?.exceptions ?? [];
  if (!summary && !affinities.length && !exceptions.length) return null;
  return (
    <footer className="taste-profile-footer" aria-label="Taste profile">
      <div className="taste-profile-overview">
        <span className="taste-profile-label">Taste profile</span>
        {summary ? <p>{summary}</p> : null}
        {affinities.length ? <span className="taste-profile-affinities">{affinities.join(" · ")}</span> : null}
      </div>
      {exceptions.length ? (
        <section className="taste-diagnostics" aria-labelledby="taste-exceptions-heading">
          <strong id="taste-exceptions-heading">Exceptions worth checking</strong>
          <ul>
            {exceptions.slice(0, 8).map((exception) => (
              <li key={`${exception.title}:${exception.tmdbId ?? ""}`}>
                {exception.title} · {exception.residual > 0 ? "more positive" : "more negative"} than its {exception.matchingFeatures.slice(0, 3).join(", ")} evidence predicted
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {observation ? (
        <span className="taste-observation">
          Observation gate: {observation.feedbackEvents}/100 feedback · {observation.laterOutcomes}/30 outcomes · {observation.feedbackReasons}/3 reasons
        </span>
      ) : null}
      </footer>
  );
}

function TastePickCard({
  pick,
  interested,
  onSelectFilm,
  onFeedback,
  openPopover,
  onOpenPopover,
  onClosePopover,
}: {
  pick: TastePick;
  interested: boolean;
  onSelectFilm: (id: string) => void;
  onFeedback: (pick: TastePick, action: "interested" | "rejected" | "seen", options?: TasteFeedbackOptions) => void;
  openPopover: OpenTastePopover | null;
  onOpenPopover: (kind: TastePopoverKind, trigger: HTMLButtonElement, openedWithKeyboard: boolean) => void;
  onClosePopover: () => void;
}) {
  const id = pickId(pick);
  const whyPopoverId = useId();
  const feedbackPopoverId = useId();
  const citedBridges = (pick.attribution?.citedPositive ?? pick.matchedFeatures ?? [])
    .filter((feature) => feature.citeable && feature.cited && feature.featureKey)
    .slice(0, 4);
  const moodSignatureCount = new Set([
    ...(pick.attribution?.moodSignature.modes ?? []).map((mode) => `mode:${mode.toLowerCase()}`),
    ...(pick.attribution?.moodSignature.thematicKeywords ?? []).map((keyword) => `keyword:${keyword.toLowerCase()}`),
  ]).size;
  const passOpen = openPopover?.kind === "pass";
  const whyOpen = openPopover?.kind === "why";
  const selectedFeedback = interested ? "interested" : passOpen ? "rejected" : null;

  function togglePopover(kind: TastePopoverKind, trigger: HTMLButtonElement, openedWithKeyboard: boolean) {
    if (openPopover?.kind === kind) {
      onClosePopover();
      return;
    }
    onOpenPopover(kind, trigger, openedWithKeyboard);
  }

  function feedbackKeyDown(event: React.KeyboardEvent<HTMLButtonElement>) {
    if (!['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp'].includes(event.key)) return;
    const controls = Array.from(
      event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="radio"]') ?? [],
    );
    const current = controls.indexOf(event.currentTarget);
    const direction = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : -1;
    const next = controls[(current + direction + controls.length) % controls.length];
    event.preventDefault();
    next?.focus();
    next?.click();
  }

  function completePass(options: TasteFeedbackOptions) {
    onClosePopover();
    onFeedback(pick, "rejected", options);
  }

  return (
    <li>
      <article className={`taste-pick${interested ? " is-interested" : ""}`}>
        <div className="taste-pick-open">
          {id ? (
            <button type="button" className="taste-pick-poster" aria-label={`View ${pick.title}`} onClick={() => onSelectFilm(id)}>
              <Poster name={pick.title} poster={pick.poster} large />
            </button>
          ) : (
            <Poster name={pick.title} poster={pick.poster} large />
          )}
          <div className="taste-pick-copy">
            {id ? (
              <button type="button" className="taste-pick-title" title={pick.title} onClick={() => onSelectFilm(id)}>
                {pick.title}
              </button>
            ) : (
              <strong title={pick.title}>{pick.title}</strong>
            )}
            <span className="taste-pick-meta muted">
              <span className={`taste-match ${matchTone(pick)}`}>{matchPercent(pick)}</span>
              {pick.watchlist ? <span className="taste-watchlist-badge">Watchlist</span> : null}
              {pick.attribution ? (
                <button
                  type="button"
                  className="taste-why-trigger"
                  aria-label={`Why ${pick.title} is a ${matchPercent(pick)}`}
                  aria-controls={whyOpen ? whyPopoverId : undefined}
                  aria-expanded={whyOpen}
                  aria-haspopup="dialog"
                  title="Why this match"
                  onClick={(event) => togglePopover("why", event.currentTarget, event.detail === 0)}
                >
                  <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8.5" /><path d="M12 10v5" /><path d="M12 7.2h.01" /></svg>
                </button>
              ) : null}
              {pick.year != null ? <span>{pick.year}</span> : null}
            </span>
            <div className="taste-fb" role="radiogroup" aria-label={`Feedback for ${pick.title}`}>
              <button
                type="button"
                role="radio"
                aria-label="Interested"
                aria-checked={selectedFeedback === "interested"}
                title="Use this as a positive Taste signal on your next refresh"
                className={`taste-fb-interest${selectedFeedback === "interested" ? " is-on" : ""}`}
                onKeyDown={feedbackKeyDown}
                onClick={() => onFeedback(pick, "interested")}
              >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path
                    d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78L12 21.23l8.84-8.84a5.5 5.5 0 0 0 0-7.78Z"
                    style={{ fill: interested ? "currentColor" : "none" }}
                  />
                </svg>
                <span>Interested</span>
              </button>
              <button
                type="button"
                role="radio"
                className="taste-fb-reject"
                aria-label="Pass"
                title="Tell Taste why this missed"
                aria-checked={selectedFeedback === "rejected"}
                aria-expanded={passOpen}
                aria-controls={passOpen ? feedbackPopoverId : undefined}
                aria-haspopup="dialog"
                onKeyDown={feedbackKeyDown}
                onClick={(event) => togglePopover("pass", event.currentTarget, event.detail === 0)}
              >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="m7 7 10 10M17 7 7 17" />
                </svg>
                <span>Pass</span>
              </button>
              <button
                type="button"
                role="radio"
                className="taste-fb-seen"
                aria-label="Seen"
                aria-checked={false}
                title="Hide this recommendation because I have already seen it"
                onKeyDown={feedbackKeyDown}
                onClick={() => onFeedback(pick, "seen")}
              >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="m5 12.5 4.5 4.5L19 7" />
                </svg>
                <span>Seen</span>
              </button>
            </div>
          </div>
        </div>
        {whyOpen && pick.attribution ? (
          <AnchoredPopover
            id={whyPopoverId}
            anchor={openPopover.trigger}
            title={`Why this ${matchPercent(pick)}`}
            openedWithKeyboard={openPopover.openedWithKeyboard}
            onClose={onClosePopover}
          >
            <TasteAttributionPanel pick={pick} onSelectFilm={onSelectFilm} />
          </AnchoredPopover>
        ) : null}
        {passOpen ? (
          <AnchoredPopover
            id={feedbackPopoverId}
            anchor={openPopover.trigger}
            title="Why did this miss?"
            openedWithKeyboard={openPopover.openedWithKeyboard}
            onClose={onClosePopover}
          >
            <section className="taste-feedback-sheet" aria-label="Tell Taste what missed">
                {citedBridges.length ? (
                  <div className="taste-feedback-bridges">
                    {citedBridges.map((feature) => (
                      <div key={feature.featureKey}>
                        <span>{sentenceCase(feature.name)}</span>
                        <button
                          type="button"
                          onClick={() => {
                            completePass({ reason: "wrong_connection", targetFeatureKey: feature.featureKey });
                          }}
                        >
                          That connection doesn't fit
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            completePass({ reason: "not_this_kind", targetFeatureKey: feature.featureKey });
                          }}
                        >
                          Not this kind of film
                        </button>
                      </div>
                    ))}
                  </div>
                ) : null}
                <div className="taste-feedback-options">
                  <button
                    type="button"
                    onClick={() => {
                      completePass({ reason: "already_seen_disliked" });
                    }}
                  >
                    I've seen it and didn't like it
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      completePass({ reason: "not_in_the_mood", moodScope: "this_movie_only" });
                    }}
                  >
                    Not now — just this film
                  </button>
                  <button
                    type="button"
                    disabled={moodSignatureCount < 2}
                    title={moodSignatureCount < 2 ? "This card has too little verified mood evidence, so Taste will only hide this movie." : undefined}
                    onClick={() => {
                      completePass({ reason: "not_in_the_mood", moodScope: "this_kind_right_now" });
                    }}
                  >
                    Not now — this kind of film
                  </button>
                </div>
            </section>
          </AnchoredPopover>
        ) : null}
      </article>
    </li>
  );
}

function TasteAttributionPanel({ pick, onSelectFilm }: { pick: TastePick; onSelectFilm: (id: string) => void }) {
  const attribution = pick.attribution;
  if (!attribution) return null;
  const evidence = attribution.citedPositive.slice(0, 4);
  const evidenceItems = (pick.evidenceItems ?? [])
    .flatMap((item) => {
      const id = item.filmId || (item.tmdbId != null ? `tmdb:${item.tmdbId}` : null);
      return id ? [{ ...item, id }] : [];
    })
    .slice(0, 3);
  return (
    <div className="taste-attribution">
      <div className="taste-attribution-summary">
        <strong>{sentenceCase(attribution.evidenceGrade)} evidence</strong>
        <span>Semantic fit {Math.round(attribution.semanticFit * 100)}%</span>
      </div>
      {evidence.length ? (
        <dl className="taste-attribution-list">
          {evidence.map((feature) => (
            <div key={feature.featureKey ?? `${feature.family}:${feature.name}`}>
              <dt>{sentenceCase(feature.name)}</dt>
              <dd>{feature.appearances} films · {Math.round(feature.confidence * 100)}%</dd>
            </div>
          ))}
        </dl>
      ) : null}
      {evidenceItems.length ? (
        <div className="taste-attribution-evidence">
          <span>Similar to titles you loved</span>
          <div>
            {evidenceItems.map((item) => (
              <button key={item.id} type="button" onClick={() => onSelectFilm(item.id)}>
                <Poster name={item.title} poster={item.poster} />
                <span>{item.title}</span>
              </button>
            ))}
          </div>
        </div>
      ) : attribution.seedFilms.length ? <p className="taste-attribution-seeds">Seeds: {attribution.seedFilms.slice(0, 3).join(" · ")}</p> : null}
    </div>
  );
}

export function TasteModelList({
  models,
  selected,
  disabled,
  onPick,
}: {
  models: TasteModelInfo[];
  selected: string;
  disabled?: boolean;
  onPick: (id: string) => void;
}) {
  if (!models.length) return null;
  return (
    <div className="taste-model-list">
      {models.map((item) => (
        <button
          key={item.id}
          type="button"
          className={selected === item.id ? "is-on" : ""}
          aria-pressed={selected === item.id}
          disabled={disabled}
          onClick={() => onPick(item.id)}
        >
          <strong>
            {item.label}
            <span>{item.context} · {item.cost}</span>
          </strong>
          <span>{item.blurb}</span>
        </button>
      ))}
    </div>
  );
}
