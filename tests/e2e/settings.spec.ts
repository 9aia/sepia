import { devices } from "playwright";
import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import {
  apiSessions,
  gotoApp,
  gotoAppMobile,
  openMobileSidebar,
  openSettings,
} from "./helpers/app.js";

const IPHONE = devices["iPhone 13"];

describe("settings", () => {
  afterAll(closeBrowser);

  e2e("settings opens from the account menu with its section nav", async () => {
    const page = await newPage();
    await gotoApp(page);
    await openSettings(page);

    const nav = page.locator('nav[aria-label="Settings sections"]');
    const labels = await nav.locator("button").allInnerTexts();
    expect(labels).toContain("General");
    expect(labels).toContain("Models");
    expect(labels).toContain("Notifications");
    await page.close();
  });

  e2e("the settings nav lays out as a horizontal row on mobile", async () => {
    const page = await newPage(IPHONE);
    const target = (await apiSessions()).find((s) => s.archived !== true);
    expect(target).toBeDefined();
    if (target === undefined) return;
    await gotoAppMobile(page, `${target.agent}:${target.id}`);
    await openMobileSidebar(page);
    await openSettings(page);

    const nav = page.locator('nav[aria-label="Settings sections"]');
    const layout = await nav.evaluate((el) => {
      const rect = el.getBoundingClientRect();
      return {
        direction: getComputedStyle(el).flexDirection,
        width: rect.width,
        height: rect.height,
      };
    });
    expect(layout.direction).toBe("row");
    expect(layout.width).toBeGreaterThan(layout.height);
    await page.close();
  });
});
