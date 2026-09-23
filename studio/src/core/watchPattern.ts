const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

export function watchPattern(dates: Array<string | null | undefined>): string | null {
  const yearsByMonth = new Map<number, Set<number>>();
  for (const raw of dates) {
    const match = raw?.match(/^(\d{4})-(\d{2})/);
    if (!match) continue;
    const year = Number(match[1]);
    const month = Number(match[2]);
    if (month < 1 || month > 12) continue;
    const years = yearsByMonth.get(month) ?? new Set<number>();
    years.add(year);
    yearsByMonth.set(month, years);
  }

  let best: { month: number; years: number } | null = null;
  for (const [month, years] of yearsByMonth) {
    if (years.size < 2) continue;
    if (!best || years.size > best.years) best = { month, years: years.size };
  }
  if (!best) return null;
  const name = MONTHS[best.month - 1];
  return best.years === 2
    ? `You've watched this in ${name} in two different years.`
    : `You usually watch this in ${name}.`;
}
