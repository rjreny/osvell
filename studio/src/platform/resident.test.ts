import { describe, expect, it } from "vitest";
import { normalizeResidentPrefs } from "./resident";

describe("normalizeResidentPrefs", () => {
  it("keeps start-in-tray only when open-with-Windows is on", () => {
    expect(
      normalizeResidentPrefs({
        launchAtLogin: false,
        startMinimized: true,
        closeMinimizes: true,
      }),
    ).toEqual({
      launchAtLogin: false,
      startMinimized: false,
      closeMinimizes: true,
    });
    expect(
      normalizeResidentPrefs({
        launchAtLogin: true,
        startMinimized: true,
        closeMinimizes: false,
      }),
    ).toEqual({
      launchAtLogin: true,
      startMinimized: true,
      closeMinimizes: false,
    });
  });
});
