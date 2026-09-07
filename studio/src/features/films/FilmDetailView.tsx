import { useEffect, useMemo, useState } from "react";
import { communityRatingOutOfFive, isHighQualityBanner } from "../../core/images";
import { log } from "../../platform/log";
import { getFilm, getFilmArtwork, setFilmArtwork } from "../../platform/filmLibrary";
import type {
  FilmCastMember,
  FilmConnection,
  FilmCrewMember,
  FilmDetail,
  FilmArtwork,
  FilmTrailer,
  LibraryItem,
  ProductionCompany,
  ViewingHistoryItem,
} from "../../platform/types/film";
import { FilmCard } from "./FilmCard";
import { Poster } from "./Poster";
import { RatingDisplay } from "./RatingDisplay";
import { Shelf } from "./Shelf";

function runtimeLabel(minutes: number | null) {
  if (!minutes) return null;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h <= 0) return `${m} min`;
  return m ? `${h}h ${m}m` : `${h}h`;
}

function formatWatchDate(iso: string | null | undefined) {
  if (!iso) return "Unknown date";
  const day = iso.slice(0, 10);
  const d = new Date(`${day}T12:00:00`);
  if (Number.isNaN(d.getTime())) return day || iso;
  return d.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

function sourceLabel(source: string) {
  const raw = source.replace(/^\d+\./, "").replace(/_/g, " ").trim();
  if (/letterboxd export/i.test(raw)) return "Letterboxd export";
  if (/letterboxd rss/i.test(raw)) return "Letterboxd RSS";
  if (/local/i.test(raw)) return "Osvell";
  return raw.replace(/\b\w/g, (c) => c.toUpperCase());
}

function initials(name: string) {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0])
    .join("")
    .toUpperCase();
}

function PeopleAvatar({ name, image }: { name: string; image: string | null }) {
  return image ? <img className="credit-avatar" src={image} alt="" /> : <span className="credit-avatar is-fallback" aria-hidden="true">{initials(name)}</span>;
}

function CastDirectory({ cast }: { cast: FilmCastMember[] }) {
  const [expanded, setExpanded] = useState(false);
  if (!cast.length) return <p className="muted">Not listed yet.</p>;
  const visibleCast = expanded ? cast : cast.slice(0, 16);
  const remaining = cast.length - visibleCast.length;
  return (
    <div className="credit-directory">
      <ul className="credit-cards" aria-label={`Cast directory, ${cast.length} members`}>
        {visibleCast.map((member) => (
          <li key={`${member.tmdbId ?? member.name}-${member.order ?? ""}`}>
            <PeopleAvatar name={member.name} image={member.profile} />
            <span><strong>{member.name}</strong>{member.character ? <small>{member.character}</small> : null}</span>
          </li>
        ))}
      </ul>
      {remaining > 0 ? (
        <button type="button" className="detail-expand" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
          {expanded ? "Show less cast" : `Show all ${cast.length} cast members`}
        </button>
      ) : null}
    </div>
  );
}

function groupCrew(crew: FilmCrewMember[]) {
  const groups = new Map<string, FilmCrewMember[]>();
  for (const member of crew) {
    const key = member.department || "Crew";
    const group = groups.get(key) ?? [];
    if (!group.some((existing) => existing.tmdbId === member.tmdbId && existing.job === member.job && existing.name === member.name)) group.push(member);
    groups.set(key, group);
  }
  return [...groups.entries()].map(([department, members]) => ({ department, members })).sort((a, b) => a.department.localeCompare(b.department));
}

function CrewDirectory({ crew }: { crew: FilmCrewMember[] }) {
  const [expanded, setExpanded] = useState(false);
  const groups = useMemo(() => groupCrew(crew), [crew]);
  if (!groups.length) return <p className="muted">Not listed yet.</p>;
  const visibleGroups = expanded ? groups : groups.slice(0, 8);
  const isLong = crew.length > 32 || groups.length > 8;
  return (
    <div className="crew-directory">
      <dl className="crew-groups">
        {visibleGroups.map((group) => {
          const members = expanded ? group.members : group.members.slice(0, 4);
          return (
            <div key={group.department} className="crew-group">
              <dt>{group.department}</dt>
              <dd>
                {members.map((member) => <span key={`${member.tmdbId ?? member.name}-${member.job}`}><strong>{member.name}</strong><small>{member.job}</small></span>)}
                {!expanded && group.members.length > members.length ? <em>+{group.members.length - members.length} more</em> : null}
              </dd>
            </div>
          );
        })}
      </dl>
      {isLong ? <button type="button" className="detail-expand" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>{expanded ? "Show less crew" : `Show all ${crew.length} crew credits`}</button> : null}
    </div>
  );
}

function Production({ companies }: { companies: ProductionCompany[] }) {
  if (!companies.length) return <p className="muted">Production companies are not listed yet.</p>;
  return (
    <ul className="company-list">
      {companies.map((company) => (
        <li key={company.tmdbId ?? company.name}>
          {company.logo ? <img src={company.logo} alt="" /> : <span className="company-mark" aria-hidden="true">{initials(company.name)}</span>}
          <span><strong>{company.name}</strong>{company.originCountry ? <small>{company.originCountry}</small> : null}</span>
        </li>
      ))}
    </ul>
  );
}

function LastRating({ viewing }: { viewing: ViewingHistoryItem }) {
  return (
    <aside className="detail-last-rating" aria-label="Your latest rating">
      <h2>Last rating</h2>
      <div className="detail-last-rating-value"><strong>{formatWatchDate(viewing.occurredAt ?? viewing.publishedAt)}</strong><RatingDisplay value={viewing.rating} compact /></div>
      <div className="detail-last-rating-source">{viewing.rewatch ? <span className="rewatch-badge">Rewatch</span> : null}<span className="muted">{sourceLabel(viewing.source)}</span></div>
    </aside>
  );
}

function ConnectionSection({ connections }: { connections: FilmConnection[] }) {
  const [expanded, setExpanded] = useState(false);
  if (!connections.length) return null;
  const visible = expanded ? connections : connections.slice(0, 6);
  return (
    <section className="detail-block detail-connections">
      <div className="detail-heading"><h2>Your connection to this film</h2><span className="muted">From your rated history</span></div>
      <ul className="connection-list">
        {visible.map((connection) => (
          <li key={connection.entityId} className={`connection-row is-${connection.tone}`}>
            <details>
              <summary>
                <span><strong>{connection.name}</strong><small>{connection.roles.join(" · ")}</small></span>
                <span className="connection-summary"><b>{connection.tone === "unknown" ? "Limited evidence" : connection.tone}</b><em>{connection.sharedCount} shared · {connection.averageRating.toFixed(1)}</em></span>
              </summary>
              <div className="connection-evidence">
                <p>{connection.confidence} · your average is based on {connection.sharedCount} shared film{connection.sharedCount === 1 ? "" : "s"}.</p>
                <ul>{connection.evidence.map((film) => <li key={film.id}><span>{film.title}</span><RatingDisplay value={film.rating} compact /></li>)}</ul>
              </div>
            </details>
          </li>
        ))}
      </ul>
      {connections.length > visible.length ? <button type="button" className="detail-expand" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>Show all {connections.length} connections</button> : null}
    </section>
  );
}

function RelatedShelf({ title, source, films, onSelect }: { title: string; source: string; films: LibraryItem[]; onSelect?: (id: string) => void }) {
  if (!films.length) return null;
  return <section className="detail-block detail-related"><Shelf title={title}>{films.map((film) => <FilmCard key={film.id} film={film} onSelect={onSelect} />)}</Shelf><p className="section-source">{source}</p></section>;
}

function youtubeEmbedUrl(key: string) {
  return `https://www.youtube-nocookie.com/embed/${encodeURIComponent(key)}?autoplay=1&rel=0&modestbranding=1`;
}

function TrailerPlayer({ trailers, onClose }: { trailers: FilmTrailer[]; onClose: () => void }) {
  const [activeKey, setActiveKey] = useState(trailers[0]?.key ?? "");
  const active = trailers.find((trailer) => trailer.key === activeKey) ?? trailers[0];

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  if (!active) return null;

  return (
    <div
      className="trailer-overlay"
      role="dialog"
      aria-modal="true"
      aria-label={`${active.name} trailer`}
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="trailer-card">
        <div className="trailer-head">
          <div>
            <h2>{active.name}</h2>
            <span className="muted">{active.type}{active.official ? " · Official" : ""}</span>
          </div>
          <button type="button" className="ghost" onClick={onClose}>Close</button>
        </div>
        <div className="trailer-frame">
          <iframe
            key={active.key}
            title={active.name}
            src={youtubeEmbedUrl(active.key)}
            allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share"
            allowFullScreen
          />
        </div>
        {trailers.length > 1 ? (
          <div className="trailer-picker" role="tablist" aria-label="Trailers">
            {trailers.map((trailer) => (
              <button
                key={trailer.key}
                type="button"
                role="tab"
                aria-selected={trailer.key === active.key}
                className={trailer.key === active.key ? "is-active" : undefined}
                onClick={() => setActiveKey(trailer.key)}
              >
                {trailer.name}
              </button>
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
}

const INITIAL_POSTER_CHOICES = 24;
const INITIAL_BACKDROP_CHOICES = 12;

function ArtworkPicker({ film, onUpdated, onStatus }: {
  film: FilmDetail;
  onUpdated: (film: FilmDetail) => void;
  onStatus: (status: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [artwork, setArtwork] = useState<FilmArtwork | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [posterUrl, setPosterUrl] = useState("");
  const [backdropUrl, setBackdropUrl] = useState("");
  const [posterLimit, setPosterLimit] = useState(INITIAL_POSTER_CHOICES);
  const [backdropLimit, setBackdropLimit] = useState(INITIAL_BACKDROP_CHOICES);

  useEffect(() => {
    setOpen(false);
    setArtwork(null);
    setError(null);
    setPosterUrl("");
    setBackdropUrl("");
    setPosterLimit(INITIAL_POSTER_CHOICES);
    setBackdropLimit(INITIAL_BACKDROP_CHOICES);
  }, [film.id]);

  async function openPicker() {
    setOpen(true);
    if (artwork || loading) return;
    setLoading(true);
    setError(null);
    try {
      const next = await getFilmArtwork(film.id);
      setArtwork(next);
      setPosterUrl(next.selectedPoster?.startsWith("https://") ? next.selectedPoster : "");
      setBackdropUrl(next.selectedBackdrop?.startsWith("https://") ? next.selectedBackdrop : "");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  async function save(poster: string | null, backdrop: string | null, message: string) {
    setSaving(true);
    setError(null);
    try {
      const updated = await setFilmArtwork(film.id, { poster, backdrop });
      onUpdated(updated);
      setArtwork((current) => current ? {
        ...current,
        selectedPoster: poster ?? current.defaultPoster,
        selectedBackdrop: backdrop ?? current.defaultBackdrop,
      } : current);
      onStatus(message);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  if (!film.tmdbId) return null;

  return (
    <section className="artwork-panel" aria-label="Artwork controls">
      <div className="artwork-panel-heading">
        <div><strong>Artwork</strong><span>Choose a high-resolution TMDB image or use an HTTPS image URL.</span></div>
        <button type="button" className="detail-expand" aria-expanded={open} onClick={() => open ? setOpen(false) : void openPicker()}>
          {open ? "Close artwork options" : "Customize artwork"}
        </button>
      </div>
      {open ? (
        <div className="artwork-panel-content">
          {loading ? <p className="muted">Loading artwork…</p> : null}
          {error ? <p className="artwork-error" role="status">{error}</p> : null}
          {artwork ? <>
            <div className="artwork-choice-group">
              <div className="detail-heading"><h2>Poster</h2><span className="muted">{artwork.posters.length} choices</span></div>
              {artwork.posters.length ? <div className="artwork-poster-grid">
                {artwork.posters.slice(0, posterLimit).map((image, index) => <button
                  type="button"
                  key={image.path}
                  className={`artwork-poster-choice${artwork.selectedPoster === image.path ? " is-selected" : ""}`}
                  aria-label={`Use poster ${index + 1}`}
                  aria-pressed={artwork.selectedPoster === image.path}
                  disabled={saving}
                  onClick={() => void save(image.path, artwork.selectedBackdrop, "Poster updated")}
                ><img src={image.url} alt="" loading="lazy" decoding="async" /></button>)}
              </div> : <p className="muted">TMDB has no high-resolution poster alternatives for this film.</p>}
              {artwork.posters.length > posterLimit ? <button type="button" className="detail-expand artwork-load-more" onClick={() => setPosterLimit((current) => current + INITIAL_POSTER_CHOICES)}>Show {Math.min(INITIAL_POSTER_CHOICES, artwork.posters.length - posterLimit)} more posters</button> : null}
            </div>
            <div className="artwork-choice-group">
              <div className="detail-heading"><h2>Hero background</h2><span className="muted">{artwork.backdrops.length} choices</span></div>
              {artwork.backdrops.length ? <div className="artwork-backdrop-grid">
                {artwork.backdrops.slice(0, backdropLimit).map((image, index) => <button
                  type="button"
                  key={image.path}
                  className={`artwork-backdrop-choice${artwork.selectedBackdrop === image.path ? " is-selected" : ""}`}
                  aria-label={`Use hero background ${index + 1}`}
                  aria-pressed={artwork.selectedBackdrop === image.path}
                  disabled={saving}
                  onClick={() => void save(artwork.selectedPoster, image.path, "Hero background updated")}
                ><img src={image.url} alt="" loading="lazy" decoding="async" /></button>)}
              </div> : <p className="muted">TMDB has no high-resolution backdrop alternatives for this film.</p>}
              {artwork.backdrops.length > backdropLimit ? <button type="button" className="detail-expand artwork-load-more" onClick={() => setBackdropLimit((current) => current + INITIAL_BACKDROP_CHOICES)}>Show {Math.min(INITIAL_BACKDROP_CHOICES, artwork.backdrops.length - backdropLimit)} more backgrounds</button> : null}
            </div>
            <div className="artwork-url-controls">
              <label>Poster image URL<input type="url" value={posterUrl} placeholder="https://…" onChange={(event) => setPosterUrl(event.target.value)} /></label>
              <label>Hero image URL<input type="url" value={backdropUrl} placeholder="https://…" onChange={(event) => setBackdropUrl(event.target.value)} /></label>
              <p className="muted artwork-url-note">Image URLs are your own choice and are not quality-checked.</p>
              <div className="artwork-url-actions">
                <button type="button" className="detail-expand" disabled={saving || (!posterUrl.trim() && !backdropUrl.trim())} onClick={() => void save(posterUrl.trim() || artwork.selectedPoster, backdropUrl.trim() || artwork.selectedBackdrop, "Custom artwork saved")}>Use image URLs</button>
                <button type="button" className="detail-expand" disabled={saving} onClick={() => void save(null, null, "Restored TMDB artwork")}>Restore TMDB artwork</button>
              </div>
            </div>
          </> : null}
        </div>
      ) : null}
    </section>
  );
}

export function FilmDetailView({ filmId, onBack, backLabel, onStatus, onSelectFilm, onArtworkChange }: {
  filmId: string;
  onBack: () => void;
  backLabel: string;
  onStatus: (s: string) => void;
  onSelectFilm?: (id: string) => void;
  onArtworkChange?: () => void;
}) {
  const [film, setFilm] = useState<FilmDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [heroImageFailed, setHeroImageFailed] = useState(false);
  const [trailerOpen, setTrailerOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setFilm(null);
    setError(null);
    setTrailerOpen(false);
    void getFilm(filmId).then((next) => !cancelled && setFilm(next)).catch((err) => {
      log("error", "film load failed", err);
      if (!cancelled) {
        setError(err instanceof Error ? err.message : String(err));
        onStatus("Could not load film");
      }
    });
    return () => { cancelled = true; };
  }, [filmId, onStatus]);

  useEffect(() => setHeroImageFailed(false), [film?.id, film?.backdrop]);

  if (error) return <div className="pad page-pad"><button type="button" className="detail-back" onClick={onBack}>{backLabel}</button><p className="muted">Could not load this film. {error}</p></div>;
  if (!film) return <p className="muted pad">Loading…</p>;

  const image = !heroImageFailed && isHighQualityBanner(film.backdrop) ? film.backdrop : null;
  const directors = film.directors?.join(" · ");
  const runtime = runtimeLabel(film.runtime);
  const community = communityRatingOutOfFive(film.tmdbVoteAverage);
  const latestViewing = film.yourHistory[0] ?? null;
  const trailers = (film.trailers ?? []).filter((trailer) => trailer.site === "YouTube" && trailer.key);

  return (
    <article className="film-detail">
      <header className={`hero detail-hero${image ? "" : " is-empty-hero"}`}>
        {image ? <img className="hero-image" src={image} alt="" onError={() => setHeroImageFailed(true)} /> : <div className="hero-image is-empty" />}
        <div className="hero-scrim" />
        <div className="hero-copy">
          <button type="button" className="detail-back" onClick={onBack}>{backLabel}</button>
          <div className="detail-identity">
            <Poster name={film.title} poster={film.poster} large className="detail-poster" />
            <div className="hero-copy-text">
              {directors ? <p className="hero-cast">{directors}</p> : null}
              <h1>{film.title}</h1>
              <p className="hero-meta"><span>{film.year ?? "Year unknown"}</span>{runtime ? <span>{runtime}</span> : null}{film.genres[0] ? <span>{film.genres[0]}</span> : null}</p>
              <div className="detail-ratings" aria-label="TMDB rating">
                <div className="rating-unit"><span className="rating-label">TMDB</span>{community != null ? <RatingDisplay value={community} compact /> : <span className="rating-unavailable">Not rated</span>}{film.tmdbVoteCount ? <small>{film.tmdbVoteCount.toLocaleString()} votes</small> : null}</div>
              </div>
              {film.tagline ? <p className="hero-lede">{film.tagline}</p> : null}
              {trailers.length ? (
                <div className="hero-actions">
                  <button type="button" className="play-btn" onClick={() => setTrailerOpen(true)}>
                    {trailers.length === 1 ? "Play trailer" : `Play trailer (${trailers.length})`}
                  </button>
                </div>
              ) : null}
            </div>
          </div>
        </div>
      </header>

      {trailerOpen && trailers.length ? <TrailerPlayer trailers={trailers} onClose={() => setTrailerOpen(false)} /> : null}

      <ArtworkPicker film={film} onUpdated={(updated) => { setFilm(updated); onArtworkChange?.(); }} onStatus={onStatus} />

      <div className="detail-body">
        <div className="detail-layout">
          <div className="detail-overview-row">
            <section className="detail-block detail-about"><h2>About</h2>{film.overview ? <p className="detail-overview">{film.overview}</p> : <p className="muted">Not enriched yet.</p>}{film.genres.length ? <p className="genre-row">{film.genres.join(" · ")}</p> : null}</section>
            {latestViewing ? <LastRating viewing={latestViewing} /> : null}
          </div>
          {film.friends.length ? <section className="detail-block detail-friends"><h2>Friends</h2><ul className="friend-chips">{film.friends.map((friend, index) => <li key={`${friend.username}-${index}`}><strong>@{friend.username}</strong><RatingDisplay value={friend.rating} compact />{friend.review ? <span className="muted">{friend.review}</span> : null}</li>)}</ul></section> : null}
          <section className="detail-block detail-cast"><div className="detail-heading"><h2>Cast</h2><span className="muted">{film.cast.length} credited</span></div><CastDirectory cast={film.cast} /></section>
          <section className="detail-block detail-crew"><div className="detail-heading"><h2>Crew</h2><span className="muted">{film.crew.length} credits</span></div><CrewDirectory crew={film.crew} /></section>
          <section className="detail-block detail-production"><h2>Production</h2><Production companies={film.companies} /></section>
          <ConnectionSection connections={film.connections} />
        </div>
        <RelatedShelf title={film.collectionName || "Related films"} source="TMDB collection / sequels" films={film.collection ?? []} onSelect={onSelectFilm} />
        <RelatedShelf title="Similar films" source="TMDB recommendations" films={film.similar} onSelect={onSelectFilm} />
      </div>
    </article>
  );
}
