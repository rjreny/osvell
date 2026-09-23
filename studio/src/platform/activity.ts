import type { JobProgress } from "./types/film";

export function quietFeedUpdate(next: JobProgress): boolean {
  return next.job === "feeds" && Boolean(next.quiet);
}

export function quietFeedNotice(next: JobProgress): string | null {
  if (!quietFeedUpdate(next) || !next.done) return null;
  const added = next.feeds?.entriesAdded ?? 0;
  if (added <= 0) return null;
  return added === 1 ? "1 new diary entry" : `${added} new diary entries`;
}

export function quietFeedShouldRefresh(next: JobProgress): boolean {
  if (!quietFeedUpdate(next) || !next.done) return false;
  const added = next.feeds?.entriesAdded ?? 0;
  return added > 0 || next.errors > 0 || Boolean(next.feeds?.pausedUntil);
}
