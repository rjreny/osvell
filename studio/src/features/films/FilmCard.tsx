import type { LibraryItem } from "../../platform/types/film";
import { Poster } from "./Poster";
import { RatingDisplay } from "./RatingDisplay";

export function FilmCard({
  film,
  onSelect,
  caption,
  showRating = true,
}: {
  film: Pick<LibraryItem, "id" | "title" | "year" | "poster" | "currentRating"> & {
    viewingCount?: number;
    matchState?: string;
  };
  onSelect?: (id: string) => void;
  caption?: string;
  showRating?: boolean;
}) {
  const seen = film.viewingCount && film.viewingCount > 1 ? `${film.viewingCount}×` : "";
  const meta = [caption || (film.year != null ? String(film.year) : ""), seen].filter(Boolean).join(" · ");

  const body = (
    <>
      <div className="film-card-poster">
        <Poster name={film.title} poster={film.poster} large />
      </div>
      <strong className="film-card-title" title={film.title}>
        {film.title}
      </strong>
      <span className="title-year">{meta || "\u00a0"}</span>
      {showRating ? <RatingDisplay value={film.currentRating} starsOnly /> : null}
    </>
  );

  if (onSelect) {
    return (
      <button type="button" className="film-card" onClick={() => onSelect(film.id)}>
        {body}
      </button>
    );
  }

  return <div className="film-card">{body}</div>;
}
