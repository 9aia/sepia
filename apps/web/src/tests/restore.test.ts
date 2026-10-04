import { describe, expect, it } from "vite-plus/test";
import { restoreSummary } from "../hooks/query/useRestore";

describe("restoreSummary", () => {
  it("counts changed files and skips", () => {
    expect(
      restoreSummary({
        restored: [
          { path: "/a", action: "written", bytes: 3 },
          { path: "/b", action: "deleted" },
          { path: "/c", action: "unchanged" },
        ],
        skipped: [{ path: "/d", reason: "drifted" }],
      }),
    ).toBe("2 files restored · 1 skipped");
  });

  it("singularizes one file and reports nothing-to-restore", () => {
    expect(
      restoreSummary({ restored: [{ path: "/a", action: "written" }], skipped: [] }),
    ).toBe("1 file restored");
    expect(restoreSummary({ restored: [], skipped: [] })).toBe("Nothing to restore");
  });
});
