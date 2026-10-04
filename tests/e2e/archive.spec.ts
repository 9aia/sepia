import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import { apiSessions, gotoApp, patchSession, sectionRows, sectionTitles } from "./helpers/app.js";

describe("archiving", () => {
  afterAll(closeBrowser);

  e2e("an archived session leaves normal views and lands in Archived last", async () => {
    const page = await newPage();
    await gotoApp(page);
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    expect(target).toBeDefined();
    if (target === undefined) return;

    try {
      await patchSession(target, { archived: true });
      await page.reload({ waitUntil: "domcontentloaded" });
      await page.waitForSelector('[data-slot="sidebar"]');
      await page.waitForTimeout(1500);

      const titles = await sectionTitles(page);
      expect(titles[titles.length - 1]).toBe("Archived");
      const archived = await sectionRows(page, "Archived");
      expect(archived.some((t) => t.includes(target.title.slice(0, 15)))).toBe(true);
      const folders = await sectionRows(page, "Folders");
      expect(folders.some((t) => t.includes(target.title.slice(0, 15)))).toBe(false);
    } finally {
      await patchSession(target, { archived: false });
    }
    await page.close();
  });
});
