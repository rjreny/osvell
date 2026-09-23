import { describe, expect, it } from "vitest";
import type { JobProgress } from "./types/film";
import { quietFeedNotice, quietFeedShouldRefresh, quietFeedUpdate } from "./activity";

function feed(partial: Partial<JobProgress>): JobProgress {
  return {
    job: "feeds",
    label: "Refreshed",
    current: 1,
    total: 1,
    posters: 0,
    errors: 0,
    done: true,
    quiet: true,
    ...partial,
  };
}

describe("quiet feed updates", () => {
  it("stays silent when a background check finds nothing new", () => {
    const next = feed({
      feeds: { selfSynced: false, friendsSynced: 0, entriesAdded: 0, skipped: true, errors: [] },
    });
    expect(quietFeedUpdate(next)).toBe(true);
    expect(quietFeedNotice(next)).toBeNull();
    expect(quietFeedShouldRefresh(next)).toBe(false);
  });

  it("names new diary entries and refreshes after a background check", () => {
    const one = feed({
      feeds: { selfSynced: true, friendsSynced: 0, entriesAdded: 1, skipped: false, errors: [] },
    });
    const many = feed({
      feeds: { selfSynced: true, friendsSynced: 0, entriesAdded: 3, skipped: false, errors: [] },
    });
    expect(quietFeedNotice(one)).toBe("1 new diary entry");
    expect(quietFeedNotice(many)).toBe("3 new diary entries");
    expect(quietFeedShouldRefresh(many)).toBe(true);
  });

  it("leaves a manual refresh visible", () => {
    const next = feed({
      quiet: false,
      feeds: { selfSynced: false, friendsSynced: 0, entriesAdded: 0, skipped: true, errors: [] },
    });
    expect(quietFeedUpdate(next)).toBe(false);
  });
});
