/** A recommendation can be shown when its release year is already over.
 *  The library stores a year, not a release date, so the current year stays hidden.
 */
export function isReleasedYear(year: number | null | undefined, now = new Date()): boolean {
  if (year == null || !Number.isFinite(year)) return false;
  return year < now.getFullYear();
}
