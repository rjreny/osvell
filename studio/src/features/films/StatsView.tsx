import { useEffect, useMemo, useState } from "react";
import { getCoverage, getLibrary, getStats } from "../../platform/filmLibrary";
import type { LibraryCoverage, LibraryItem, StatsBucket, StatsSnapshot } from "../../platform/types/film";
import { FilmCard } from "./FilmCard";
import { Shelf } from "./Shelf";

function Histogram({
  buckets,
  className = "",
  fill = false,
  showValues = true,
  formatLabel,
}: {
  buckets: Pick<StatsBucket, "label" | "count">[];
  className?: string;
  fill?: boolean;
  showValues?: boolean;
  formatLabel?: (label: string, index: number, total: number) => string;
}) {
  const max = Math.max(1, ...buckets.map(({ count }) => count));
  return (
    <div className="stats-chart">
      <ol
        className={`stats-histogram ${fill ? "is-fill" : ""} ${className}`}
        style={fill ? { gridTemplateColumns: `repeat(${Math.max(1, buckets.length)}, minmax(0, 1fr))` } : undefined}
      >
        {buckets.map(({ label, count }, index) => (
          <li key={label} aria-label={`${label}: ${count}`}>
            <div className="stats-histogram-plot">
              <div className="stats-histogram-column" style={{ height: `${Math.max(count ? 7 : 0, (count / max) * 100)}%` }}>
                {showValues ? <strong>{count}</strong> : null}
                <i />
              </div>
            </div>
            <span>{formatLabel ? formatLabel(label, index, buckets.length) : label}</span>
          </li>
        ))}
      </ol>
    </div>
  );
}

function formatHours(minutes: number) {
  if (!minutes) return "—";
  const hours = Math.round(minutes / 60);
  return `${hours.toLocaleString()}h`;
}

function activityMonthLabel(label: string, index: number, total: number) {
  if (index % 3 !== 0 && index !== total - 1) return "";
  if (!/^\d{4}-\d{2}$/.test(label)) return "";
  const [year, month] = label.split("-");
  return `${month}/${year.slice(2)}`;
}

export function StatsView({ onSelectFilm }: { onSelectFilm: (id: string) => void }) {
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [coverage, setCoverage] = useState<LibraryCoverage | null>(null);
  const [snapshot, setSnapshot] = useState<StatsSnapshot | null>(null);

  useEffect(() => {
    void (async () => {
      const [page, c, stats] = await Promise.all([
        getLibrary({ limit: 10000, sort: "rating" }),
        getCoverage(),
        getStats().catch(() => null),
      ]);
      setItems(page.items);
      setCoverage(c);
      setSnapshot(stats);
    })();
  }, []);

  const distribution = useMemo(() => {
    const buckets = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    items.forEach((f) => {
      if (f.currentRating == null) return;
      const idx = Math.min(9, Math.max(0, Math.round(f.currentRating * 2) - 1));
      buckets[idx] += 1;
    });
    return buckets;
  }, [items]);

  const decades = useMemo(() => {
    const map = new Map<number, number>();
    items.forEach((f) => {
      if (!f.year) return;
      const d = Math.floor(f.year / 10) * 10;
      if (d < 1920 || d > 2020) return;
      map.set(d, (map.get(d) ?? 0) + 1);
    });
    return Array.from(
      { length: 10 },
      (_, index) => {
        const decade = 1930 + index * 10;
        return [decade, map.get(decade) ?? 0] as const;
      },
    );
  }, [items]);

  const maxBucket = Math.max(1, ...distribution);
  const ratings = items.filter((film) => film.currentRating != null);
  const averageRating = ratings.length
    ? ratings.reduce((sum, film) => sum + (film.currentRating ?? 0), 0) / ratings.length
    : null;
  const fiveStarCount = ratings.filter((film) => film.currentRating === 5).length;
  const ratingCoverage = items.length ? Math.round((ratings.length / items.length) * 100) : 0;
  const viewingMonths = snapshot?.viewingMonths ?? Array.from({ length: 24 }, (_, index) => ({
    label: `month-${index + 1}`,
    count: 0,
  }));
  const activityTotal = viewingMonths.reduce((sum, month) => sum + month.count, 0);
  const genres = snapshot?.genres ?? [];
  const maxGenreCount = Math.max(1, ...genres.map((genre) => genre.count));
  const rankedDecades = [...decades]
    .filter(([, count]) => count > 0)
    .sort((a, b) => b[1] - a[1] || b[0] - a[0]);
  const topRated = [...ratings]
    .sort((a, b) => (b.currentRating ?? 0) - (a.currentRating ?? 0) || b.viewingCount - a.viewingCount)
    .slice(0, 12);
  const mostRewatched = [...items]
    .filter((film) => film.viewingCount > 1)
    .sort((a, b) => b.viewingCount - a.viewingCount || (b.currentRating ?? 0) - (a.currentRating ?? 0))
    .slice(0, 12);
  const ratingBuckets = distribution.map((count, index) => ({
    label: ((index + 1) / 2).toFixed(1).replace(".0", ""),
    count,
  }));

  return (
    <div className="stats-page page-pad">
      <header className="page-head">
        <div>
          <h1>Stats</h1>
          <p className="muted">Your log, as numbers</p>
        </div>
        <span className="stats-scope">All time</span>
      </header>
      <ol className="stats-overview" aria-label="Viewing overview">
        <li>
          <strong>{coverage?.uniqueMovies ?? items.length}</strong>
          <span>Films</span>
        </li>
        <li>
          <strong>{coverage?.totalViewings ?? 0}</strong>
          <span>Watches</span>
          <small>{snapshot?.rewatchCount ?? 0} rewatches</small>
        </li>
        <li>
          <strong>{formatHours(snapshot?.totalRuntimeMinutes ?? 0)}</strong>
          <span>Hours watched</span>
        </li>
        <li>
          <strong>{averageRating?.toFixed(1) ?? "—"}</strong>
          <span>Average rating</span>
          <small>{ratings.length} rated</small>
        </li>
      </ol>
      {mostRewatched.length ? (
        <section className="stats-shelf">
          <Shelf title="Most rewatched">
            {mostRewatched.map((film) => (
              <FilmCard key={film.id} film={film} caption={`${film.viewingCount}× watched`} onSelect={onSelectFilm} />
            ))}
          </Shelf>
        </section>
      ) : null}
      <div className="stats-viz-row">
        <section className="stats-section stats-activity">
          <header className="stats-section-head">
            <h2>Watching activity</h2>
            <p>{activityTotal ? "Last 24 months" : "0 logs in the last 24 months"}</p>
          </header>
          <Histogram buckets={viewingMonths} className="is-activity" fill showValues={false} formatLabel={activityMonthLabel} />
        </section>
        <section className="stats-section stats-ratings">
          <header className="stats-section-head">
            <h2>Ratings</h2>
            <p>{ratings.length} films rated · most often {((distribution.indexOf(maxBucket) + 1) / 2).toFixed(1).replace(".0", "")} stars</p>
          </header>
          <Histogram buckets={ratingBuckets} className="is-ratings" fill />
          <dl className="stats-facts">
            <div><dt>Average</dt><dd>{averageRating?.toFixed(1) ?? "—"}</dd></div>
            <div><dt>Five stars</dt><dd>{fiveStarCount}</dd></div>
            <div><dt>Rated</dt><dd>{ratingCoverage}%</dd></div>
          </dl>
        </section>
      </div>
      <div className="stats-rank-row">
        <section className="stats-section stats-genres">
          <header className="stats-section-head">
            <h2>Your genres</h2>
            <p>{snapshot?.metadataMovies ?? 0} enriched films watched</p>
          </header>
          {genres.length ? (
            <ol className="stats-rank-list stats-genre-list">
              {genres.map((genre) => (
                <li key={genre.label}>
                  <div className="stats-rank-copy">
                    <strong>{genre.label}</strong>
                    <span className="stats-genre-bar" aria-hidden="true">
                      <i style={{ width: `${(genre.count / maxGenreCount) * 100}%` }} />
                    </span>
                  </div>
                  <small className="stats-rank-meta">
                    <span>{genre.count} {genre.count === 1 ? "film" : "films"}</span>
                    {genre.averageRating != null ? <span>{genre.averageRating.toFixed(1)} avg</span> : null}
                  </small>
                </li>
              ))}
            </ol>
          ) : <p className="stats-empty">No enriched viewing data yet.</p>}
        </section>
        <section className="stats-section stats-decades">
          <header className="stats-section-head">
            <h2>Decades</h2>
            <p>{rankedDecades.length} represented</p>
          </header>
          {rankedDecades.length ? (
            <ol className="stats-rank-list stats-decade-list">
              {rankedDecades.map(([decade, count]) => (
                <li key={decade}>
                  <strong>{decade}s</strong>
                  <span>{count} {count === 1 ? "film" : "films"}</span>
                </li>
              ))}
            </ol>
          ) : <p className="stats-empty">No release decades yet.</p>}
        </section>
      </div>
      {topRated.length ? (
        <section className="stats-shelf stats-shelf-secondary">
          <Shelf title="Highest rated">
            {topRated.map((film) => (
              <FilmCard key={film.id} film={film} onSelect={onSelectFilm} />
            ))}
          </Shelf>
        </section>
      ) : null}
    </div>
  );
}
