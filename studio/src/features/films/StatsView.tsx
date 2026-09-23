import { useEffect, useMemo, useState, type ReactNode } from "react";
import { getCoverage, getLibrary, getStats } from "../../platform/filmLibrary";
import type { LibraryCoverage, LibraryItem, PersonStat, StatsBucket, StatsSnapshot } from "../../platform/types/film";
import { Menu } from "../ui/Menu";

const TABS = [
  { id: "activity", label: "Activity" },
  { id: "ratings", label: "Ratings" },
  { id: "genres", label: "Genres" },
  { id: "decades", label: "Decades" },
  { id: "people", label: "People" },
] as const;

const ROLES = [
  { id: "director", label: "Directors" },
  { id: "writer", label: "Writers" },
  { id: "cinematographer", label: "Cinematography" },
  { id: "cast", label: "Cast" },
] as const;

type TabId = (typeof TABS)[number]["id"];
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

export function StatsView({ onSelectFilm: _onSelectFilm }: { onSelectFilm: (id: string) => void }) {
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [coverage, setCoverage] = useState<LibraryCoverage | null>(null);
  const [snapshot, setSnapshot] = useState<StatsSnapshot | null>(null);
  const [tab, setTab] = useState<TabId>("activity");
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
  const fiveStarCount = ratings.filter((film) => film.currentRating === 5).length;
  const ratingCoverage = items.length ? Math.round((ratings.length / items.length) * 100) : 0;
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
  const ratingsSummary = `${ratings.length} films rated · most often ${usual} stars · ${fiveStarCount} five-star · ${ratingCoverage}% rated`;

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

      <div className="stats-tabs" role="tablist" aria-label="Stats views">
        {TABS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={tab === item.id}
            className={`stats-tab${tab === item.id ? " is-on" : ""}`}
            onClick={() => setTab(item.id)}
          >
            {item.label}
          </button>
        ))}
      </div>

      <div className="stats-panel" role="tabpanel">
        {tab === "activity" ? (
          <StatsPane title="Watching activity" note={activityTotal ? "Last 24 months" : "0 logs in the last 24 months"}>
            <Histogram buckets={viewingMonths} className="is-activity" formatLabel={activityMonthLabel} />
          </StatsPane>
        ) : null}

        {tab === "ratings" ? (
          <StatsPane title="Ratings" note={ratingsSummary}>
            <Histogram buckets={ratingBuckets} className="is-ratings" />
          </StatsPane>
        ) : null}

        {tab === "genres" ? (
          <StatsPane title="Your genres" note={`${snapshot?.metadataMovies ?? 0} enriched films watched`} className="stats-genres">
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
          </StatsPane>
        ) : null}

        {tab === "decades" ? (
          <StatsPane title="Decades" note={`${decades.length} represented`} className="stats-decades">
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
          </StatsPane>
        ) : null}

        {tab === "people" ? (
          <StatsPane
            title="People"
            note="Who you keep returning to"
            className="stats-people"
            control={<Menu label="Role" value={role} options={[...ROLES]} onChange={setRole} />}
          >
            <PeopleList people={people} />
          </StatsPane>
        ) : null}
      </div>
    </div>
  );
}

function StatsPane({
  title,
  note,
  className = "",
  control,
  children,
}: {
  title: string;
  note: string;
  className?: string;
  control?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className={`stats-section${className ? ` ${className}` : ""}`}>
      <header className="stats-section-head">
        <div>
          <h2>{title}</h2>
          <p>{note}</p>
        </div>
        {control}
      </header>
      <div className="stats-well">{children}</div>
    </section>
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
