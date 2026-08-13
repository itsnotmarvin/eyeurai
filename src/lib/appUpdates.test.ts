import { describe, expect, it } from "vitest";

import { isNewerAppVersion } from "./appUpdates";

describe("isNewerAppVersion", () => {
  it("accepts only a genuinely newer semantic version", () => {
    expect(isNewerAppVersion("1.2.2", "1.2.1")).toBe(true);
    expect(isNewerAppVersion("2.0.0", "1.99.99")).toBe(true);
    expect(isNewerAppVersion("1.2.1", "1.2.1")).toBe(false);
    expect(isNewerAppVersion("1.2.0", "1.2.1")).toBe(false);
  });

  it("handles prereleases and rejects malformed feed versions", () => {
    expect(isNewerAppVersion("1.2.2", "1.2.2-beta.2")).toBe(true);
    expect(isNewerAppVersion("1.2.2-beta.2", "1.2.2-beta.1")).toBe(true);
    expect(isNewerAppVersion("1.2.2-beta.1", "1.2.2-beta")).toBe(true);
    expect(isNewerAppVersion("1.2.2-beta.1", "1.2.2")).toBe(false);
    expect(isNewerAppVersion("latest", "1.2.1")).toBe(false);
  });
});
