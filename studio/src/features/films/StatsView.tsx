import { useEffect, useMemo, useState } from "react";
import { getCoverage, getLibrary, getStats } from "../../platform/filmLibrary";
import type { LibraryCoverage, LibraryItem, PersonStat, StatsBucket, StatsSnapshot } from "../../platform/types/film";
import { Menu } from "../ui/Menu";
import { FilmCard } from "./FilmCard";
import { Shelf } from "./Shelf";

const ROLES = [
  { id: "director", label: "Directors" },
  { id: "writer", label: "Writers" },
  { id: "cinematographer", label: "Cinematography" },
  { id: "cast", label: "Cast" },
] as const;

type RoleId = (typeof ROLES)[number]["id"];

function Histogram({
  buckets,
  className = "",
  formatLabel,
}: {
  buckets: Pick<StatsBucket, "label" | "count">[];
  className?: string;
  formatLabel?: (label: string, index: number, total: number) => string;
}) {
  const max = Math.max(1, ...buckets.map(({ count }) => count));
  return (
    <div className="stats-chart">
      <ol
        className={`stats-histogram is-fill ${className}`}
        style={{ gridTemplateColumns: `repeat(${Math.max(1, buckets.length)}, minmax(0, 1fr))` }}
      >
        {buckets.map(({ label, count }, index) => (
          <li key={label} aria-label={`${formatLabel ? formatLabel(label, index, buckets.length) || label : label}: ${count}`}>
            <div className="stats-histogram-plot">
              <div className="stats-histogram-column" style={{ height: `${Math.max(count ? 7 : 0, (count / max) * 100)}%` }}>
                {className.includes("is-activity") ? null : <strong>{count}</strong>}
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

function fullMonthLabel(label: string) {
  if (!/^\d{4}-\d{2}$/.test(label)) return label;
  const [year, month] = label.split("-").map(Number);
  return new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" }).format(
    new Date(year, month - 1, 1),
  );
}

function RankList({
  rows,
}: {
  rows: { key: string; label: string; count: number; detail: string; extra?: string | null }[];
}) {
  const max = Math.max(1, ...rows.map((row) => row.count));
  return (
    <ol className="stats-rank-list">
      {rows.map((row) => (
        <li key={row.key}>
          <div className="stats-rank-copy">
            <strong>{row.label}</strong>
            <span className="stats-genre-bar" aria-hidden="true">
              <i style={{ width: `${(row.count / max) * 100}%` }} />
            </span>
          </div>
          <small className="stats-rank-meta">
            <span>{row.detail}</span>
            {row.extra ? <span>{row.extra}</span> : null}
          </small>
        </li>
      ))}
    </ol>
  );
}

function filmCount(count: number) {
  return `${count} ${count === 1 ? "film" : "films"}`;
}

function StatsInsights({
  items,
}: {
  items: { label: string; value: string; note?: string }[];
}) {
  return (
    <dl className="stats-insights">
      {items.map((item) => (
        <div key={item.label}>
          <dt>{item.label}</dt>
          <dd>{item.value}</dd>
          {item.note ? <small>{item.note}</small> : null}
        </div>
      ))}
    </dl>
  );
}

export function StatsView({ onSelectFilm }: { onSelectFilm: (id: string) => void }) {
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [coverage, setCoverage] = useState<LibraryCoverage | null>(null);
  const [snapshot, setSnapshot] = useState<StatsSnapshot | null>(null);
  const [role, setRole] = useState<RoleId>("director");

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
      if (d < 1890 || d > 2030) return;
      map.set(d, (map.get(d) ?? 0) + 1);
    });
    return [...map.entries()].sort((a, b) => b[1] - a[1] || b[0] - a[0]);
  }, [items]);

  const maxBucket = Math.max(1, ...distribution);
  const ratings = items.filter((film) => film.currentRating != null);
  const averageRating = ratings.length
    ? ratings.reduce((sum, film) => sum + (film.currentRating ?? 0), 0) / ratings.length
    : null;
  const viewingMonths = snapshot?.viewingMonths ?? Array.from({ length: 24 }, (_, index) => ({
    label: `month-${index + 1}`,
    count: 0,
  }));
  const activityTotal = viewingMonths.reduce((sum, month) => sum + month.count, 0);
  const genres = snapshot?.genres ?? [];
  const people = (snapshot?.people ?? []).filter((person) => person.role === role);
  const ratingBuckets = distribution.map((count, index) => ({
    label: ((index + 1) / 2).toFixed(1).replace(".0", ""),
    count,
  }));
  const usual = ((distribution.indexOf(maxBucket) + 1) / 2).toFixed(1).replace(".0", "");
  const ratingsSummary = `${ratings.length} rated · most often ${usual}`;
  const activeMonths = viewingMonths.filter((month) => month.count > 0);
  const busiestMonth = activeMonths.reduce<Pick<StatsBucket, "label" | "count"> | null>(
    (best, month) => (!best || month.count > best.count ? month : best),
    null,
  );
  const lovedCount = ratings.filter((film) => (film.currentRating ?? 0) >= 4).length;
  const topGenre = genres[0] ?? null;
  const highestRatedGenre = [...genres]
    .filter((genre) => genre.averageRating != null && genre.count >= 3)
    .sort((a, b) => (b.averageRating ?? 0) - (a.averageRating ?? 0))[0] ?? null;
  const topDecade = decades[0] ?? null;
  const oldestDecade = [...decades].sort((a, b) => a[0] - b[0])[0] ?? null;
  const rewatched = useMemo(
    () =>
      [...items]
        .filter((film) => film.viewingCount > 1)
        .sort((a, b) => b.viewingCount - a.viewingCount || a.title.localeCompare(b.title))
        .slice(0, 8),
    [items],
  );

  const genreNote = topGenre
    ? `${topGenre.label} leads${highestRatedGenre ? ` · ${highestRatedGenre.label} rates highest` : ""}`
    : "No enriched viewing data yet";
  const decadeNote = topDecade
    ? `${topDecade[0]}s lead${oldestDecade ? ` · from the ${oldestDecade[0]}s` : ""}`
    : "No release decades yet";

  return (
    <div className="stats-page page-pad">
      <div className="utility-canvas">
      <header className="page-head">
        <div>
          <h1>Stats</h1>
          <p className="muted">Your viewing history, at a glance</p>
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

      {rewatched.length ? (
        <Shelf className="stats-rewatch" title="Most rewatched">
          {rewatched.map((film) => (
            <FilmCard
              key={film.id}
              film={{
                id: film.id,
                title: film.title,
                year: film.year,
                poster: film.poster,
                currentRating: film.currentRating,
              }}
              caption={`${film.viewingCount} watches`}
              onSelect={onSelectFilm}
            />
          ))}
        </Shelf>
      ) : null}

      <div className="stats-pair">
        <section className="stats-section">
          <header className="stats-section-head">
            <div>
              <h2>Watching activity</h2>
              <p>{activityTotal ? "Last 24 months" : "0 logs in the last 24 months"}</p>
            </div>
          </header>
          <div className="stats-analysis-layout">
            <Histogram buckets={viewingMonths} className="is-activity" formatLabel={activityMonthLabel} />
            <StatsInsights
              items={[
                { label: "Recent watches", value: activityTotal.toLocaleString(), note: "Across the last 24 months" },
                { label: "Active months", value: `${activeMonths.length} of ${viewingMonths.length}` },
                {
                  label: "Busiest month",
                  value: busiestMonth ? fullMonthLabel(busiestMonth.label) : "—",
                  note: busiestMonth ? `${busiestMonth.count} watches` : "No recent activity",
                },
              ]}
            />
          </div>
        </section>
        <section className="stats-section stats-ratings">
          <header className="stats-section-head">
            <div>
              <h2>Ratings</h2>
              <p>{ratingsSummary}</p>
            </div>
          </header>
          <div className="stats-analysis-layout">
            <Histogram buckets={ratingBuckets} className="is-ratings" />
            <StatsInsights
              items={[
                { label: "Typical rating", value: `${usual} stars` },
                { label: "Loved", value: filmCount(lovedCount), note: "Rated 4 stars or higher" },
                { label: "Not rated", value: filmCount(Math.max(0, items.length - ratings.length)) },
              ]}
            />
          </div>
        </section>
      </div>

      <div className="stats-pair">
        <section className="stats-section stats-genres">
          <header className="stats-section-head">
            <div>
              <h2>Genres</h2>
              <p>{genreNote}</p>
            </div>
          </header>
          {genres.length ? (
            <RankList
              rows={genres.map((genre) => ({
                key: genre.label,
                label: genre.label,
                count: genre.count,
                detail: filmCount(genre.count),
                extra: genre.averageRating != null ? `${genre.averageRating.toFixed(1)} avg` : null,
              }))}
            />
          ) : (
            <p className="stats-empty">No enriched viewing data yet.</p>
          )}
        </section>
        <section className="stats-section stats-decades">
          <header className="stats-section-head">
            <div>
              <h2>Decades</h2>
              <p>{decadeNote}</p>
            </div>
          </header>
          {decades.length ? (
            <RankList
              rows={decades.map(([decade, count]) => ({
                key: String(decade),
                label: `${decade}s`,
                count,
                detail: filmCount(count),
              }))}
            />
          ) : (
            <p className="stats-empty">No release decades yet.</p>
          )}
        </section>
      </div>

      <section className="stats-section stats-people">
        <header className="stats-section-head">
          <div>
            <h2>People</h2>
            <p>Who you keep returning to</p>
          </div>
          <Menu label="Role" value={role} options={[...ROLES]} onChange={setRole} />
        </header>
        <PeopleList people={people} />
      </section>
      </div>
    </div>
  );
}

function PeopleList({ people }: { people: PersonStat[] }) {
  if (!people.length) {
    return <p className="stats-empty">No enriched credits for this role yet.</p>;
  }
  return (
    <RankList
      rows={people.map((person) => ({
        key: `${person.role}:${person.name}`,
        label: person.name,
        count: person.count,
        detail: filmCount(person.count),
        extra: person.averageRating != null ? `${person.averageRating.toFixed(1)} avg` : null,
      }))}
    />
  );
}
