import { useEffect, useMemo, useState } from "react";
import { isHighQualityBanner } from "../../core/images";
import { isReleasedYear } from "../../core/released";
import { getFilm, tasteGet } from "../../platform/filmLibrary";
import type { FilmDetail, HomeViewModel, LibraryItem, SeriesProgress, TastePick } from "../../platform/types/film";
import { FilmCard } from "./FilmCard";
import { RatingDisplay } from "./RatingDisplay";
import { Shelf, ShelfTrack } from "./Shelf";

function heroSrc(film: LibraryItem, detail: FilmDetail | null) {
  const banner = detail?.backdrop || film.backdrop;
  return isHighQualityBanner(banner) ? banner : null;
}

function trimOverview(text: string | null | undefined) {
  if (!text) return null;
  const words = text.trim().split(/\s+/);
  if (words.length <= 16) return text.trim();
  return `${words.slice(0, 16).join(" ")}…`;
}

function runtimeLabel(minutes: number | null | undefined) {
  if (!minutes) return null;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h <= 0) return `${m} min`;
  return m ? `${h}h ${m}m` : `${h}h`;
}

export function HomeView({
  home,
  onOpenFilms,
  onOpenFriends,
  onSelectFilm,
}: {
  home: HomeViewModel | null;
  onOpenFilms: () => void;
  onOpenFriends: () => void;
  onSelectFilm: (id: string) => void;
}) {
  const slides = useMemo(() => {
    const recent = home?.recent ?? [];
    const banners = recent.filter((film) => isHighQualityBanner(film.backdrop));
    return (banners.length ? banners : recent).slice(0, 6);
  }, [home]);
  const [index, setIndex] = useState(0);
  const [detail, setDetail] = useState<FilmDetail | null>(null);
  const [upNext, setUpNext] = useState<TastePick[] | null>(null);
  const featured = slides[index] ?? home?.topRated[0] ?? null;

  useEffect(() => {
    setIndex(0);
  }, [home]);

  useEffect(() => {
    if (!featured) {
      setDetail(null);
      return;
    }
    let cancelled = false;
    void getFilm(featured.id)
      .then((next) => {
        if (!cancelled) setDetail(next);
      })
      .catch(() => {
        if (!cancelled) setDetail(null);
      });
    return () => {
      cancelled = true;
    };
  }, [featured?.id]);

  useEffect(() => {
    let cancelled = false;
    void tasteGet()
      .then((state) => {
        if (cancelled) return;
        const report = state.report;
        const picks = report?.picks?.length
          ? report.picks
          : [...(report?.newPicks ?? []), ...(report?.watchlistPicks ?? [])];
        setUpNext(picks.filter((pick) => isReleasedYear(pick.year)).slice(0, 16));
      })
      .catch(() => {
        if (!cancelled) setUpNext([]);
      });
    return () => {
      cancelled = true;
    };
  }, [home]);

  useEffect(() => {
    if (slides.length < 2) return;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce) return;
    const timer = window.setInterval(() => {
      setIndex((i) => (i + 1) % slides.length);
    }, 8000);
    return () => window.clearInterval(timer);
  }, [slides.length]);

  if (!home) {
    return <p className="muted pad">Loading your library…</p>;
  }

  const image = featured ? heroSrc(featured, detail) : null;
  const castLine =
    (detail?.directors?.length ? detail.directors : detail?.cast.map((member) => member.name) ?? []).slice(0, 3).join("  ").toUpperCase();
  const overview = trimOverview(detail?.overview || featured?.overview);
  const runtime = runtimeLabel(detail?.runtime);
  const genre = detail?.genres[0];

  return (
    <div className="home-cinema">
      {featured ? (
        <section className="hero">
          {image ? (
            <>
              <img className="hero-image hero-image-backdrop" src={image} alt="" />
              <img className="hero-image hero-image-main" src={image} alt="" />
            </>
          ) : (
            <div className="hero-image is-empty" />
          )}
          <div className="hero-scrim" />
          <div className="hero-copy">
            {castLine ? <p className="hero-cast">{castLine}</p> : null}
            <h1>{featured.title}</h1>
            <p className="hero-meta">
              {featured.year ? <span>{featured.year}</span> : null}
              {runtime ? <span>{runtime}</span> : null}
              {genre ? <span>{genre}</span> : null}
              <RatingDisplay value={featured.currentRating} compact />
            </p>
            {overview ? <p className="hero-lede">{overview}</p> : null}
          </div>
          {slides.length > 1 ? (
            <div className="hero-dots" role="tablist" aria-label="Featured films">
              {slides.map((film, i) => (
                <button
                  key={film.id}
                  type="button"
                  role="tab"
                  aria-selected={i === index}
                  className={i === index ? "is-on" : ""}
                  onClick={() => setIndex(i)}
                >
                  <span className="sr-only">{film.title}</span>
                </button>
              ))}
            </div>
          ) : null}
        </section>
      ) : (
        <section className="hero is-empty-hero">
          <div className="hero-copy">
            <h1>Your log is waiting</h1>
            <p className="hero-lede">Import a Letterboxd export or connect your public diary to fill this shelf.</p>
            <div className="hero-actions">
              <button type="button" className="play-btn" onClick={onOpenFilms}>
                All films
              </button>
            </div>
          </div>
        </section>
      )}

      <div className="home-shelves">
        <Shelf
          title="Recent from your log"
          action={
            <button type="button" className="text-btn" onClick={onOpenFilms}>
              All films
            </button>
          }
          empty={
            home.recent.length ? undefined : (
              <p className="muted">Import your export or connect RSS to fill this shelf.</p>
            )
          }
        >
          {home.recent.map((film) => (
            <FilmCard key={film.id} film={film} onSelect={onSelectFilm} />
          ))}
        </Shelf>

        {upNext && (home.series || upNext.length) ? (
          <NextShelf series={home.series} picks={upNext} onSelectFilm={onSelectFilm} />
        ) : null}

        {home.thisMonth?.length ? (
          <Shelf title="This time of year">
            {home.thisMonth.map((film) => (
              <FilmCard
                key={film.id}
                film={film}
                caption={`${film.years} years`}
                onSelect={onSelectFilm}
              />
            ))}
          </Shelf>
        ) : null}

        {home.topRated.length ? (
          <Shelf title="Top rated">
            {home.topRated.map((film) => (
              <FilmCard key={film.id} film={film} onSelect={onSelectFilm} />
            ))}
          </Shelf>
        ) : null}

        <Shelf
          title="Friends just rated"
          action={
            <button type="button" className="text-btn" onClick={onOpenFriends}>
              Manage
            </button>
          }
          empty={
            home.friendFeed.length ? undefined : (
              <p className="muted">Add friends by Letterboxd username to see their public ratings.</p>
            )
          }
        >
            {home.friendFeed.slice(0, 12).map((entry, idx) => (
              <FilmCard
                key={`${entry.username}-${entry.title}-${idx}`}
                film={{
                  id: entry.filmId || `${entry.username}-${idx}`,
                  title: entry.title,
                  year: entry.year,
                  poster: entry.poster,
                  currentRating: entry.rating,
                }}
                caption={`@${entry.username}`}
                onSelect={entry.filmId ? onSelectFilm : undefined}
              />
            ))}
        </Shelf>
      </div>
    </div>
  );
}

function NextShelf({
  series,
  picks,
  onSelectFilm,
}: {
  series: SeriesProgress | null | undefined;
  picks: TastePick[];
  onSelectFilm: (id: string) => void;
}) {
  const [choice, setChoice] = useState<"next" | "series" | null>(null);
  const active = choice === "series" && series ? "series" : picks.length ? "next" : series ? "series" : "next";
  const nowYear = new Date().getFullYear();
  const nextId = series?.parts.find(
    (part) => !part.watched && (part.year == null || part.year <= nowYear),
  )?.id;

  return (
    <section className="shelf next-shelf" aria-label={active === "series" && series ? `Finish ${series.name}` : "Watch Next"}>
      <header className="shelf-head">
        <div className="next-tabs" role="tablist" aria-label="What to watch">
          {picks.length ? (
            <button
              type="button"
              role="tab"
              aria-selected={active === "next"}
              className={`next-tab${active === "next" ? " is-on" : ""}`}
              onClick={() => setChoice("next")}
            >
              Watch Next
            </button>
          ) : null}
          {series ? (
            <button
              type="button"
              role="tab"
              aria-selected={active === "series"}
              className={`next-tab${active === "series" ? " is-on" : ""}`}
              onClick={() => setChoice("series")}
            >
              Finish {series.name}
            </button>
          ) : null}
        </div>
        {active === "series" && series ? (
          <span className="muted">
            {series.watched} of {series.total}
          </span>
        ) : null}
      </header>
      <ShelfTrack key={active}>
        {active === "next"
          ? picks.map((pick) => {
              const id = pick.filmId || (pick.tmdbId ? `tmdb:${pick.tmdbId}` : "");
              if (!id) return null;
              return (
                <FilmCard
                  key={id}
                  film={{
                    id,
                    title: pick.title,
                    year: pick.year,
                    poster: pick.poster,
                    currentRating: null,
                  }}
                  onSelect={onSelectFilm}
                  showRating={false}
                />
              );
            })
          : series?.parts.map((part, index) => {
              const upcoming = part.year != null && part.year > nowYear;
              const isNext = !part.watched && part.id === nextId;
              const state = part.watched ? "Watched" : isNext ? "Next" : upcoming ? "Later" : null;
              return (
                <FilmCard
                  key={`${part.id}-${index}`}
                  film={{
                    id: part.id,
                    title: part.title,
                    year: part.year,
                    poster: part.poster,
                    currentRating: part.currentRating,
                  }}
                  caption={state ? `${index + 1} · ${state}` : String(index + 1)}
                  showRating={part.watched}
                  onSelect={part.openable ? onSelectFilm : undefined}
                />
              );
            })}
      </ShelfTrack>
    </section>
  );
}
