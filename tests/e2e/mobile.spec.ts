import { devices } from "playwright";
import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import { apiSessions, gotoAppMobile, openMobileSidebar, openSession } from "./helpers/app.js";

const IPHONE = devices["iPhone 13"];

const firstVisibleSession = async () => (await apiSessions()).find((s) => s.archived !== true);

describe("mobile layout", () => {
  afterAll(closeBrowser);

  e2e("the sidebar trigger opens a ~90vw sheet and tapping a row closes it", async () => {
    const page = await newPage(IPHONE);
    const target = await firstVisibleSession();
    expect(target).toBeDefined();
    if (target === undefined) return;
    await gotoAppMobile(page);
    await openMobileSidebar(page);

    const sheet = page.locator('[data-slot="sidebar"][data-mobile="true"]');
    const box = await sheet.boundingBox();
    const viewportWidth = page.viewportSize()?.width ?? 0;
    expect(box).not.toBeNull();
    if (box === null) return;
    // SIDEBAR_WIDTH_MOBILE is 90dvw.
    expect(box.width).toBeGreaterThan(viewportWidth * 0.8);
    expect(box.width).toBeLessThanOrEqual(viewportWidth);

    await openSession(page, target.title);
    await sheet.waitFor({ state: "detached", timeout: 10_000 });
    const selected = new URL(page.url()).searchParams.get("session");
    expect(selected).toMatch(/^.+:.+$/);
    await page.close();
  });

  e2e("the session details drawer opens from the chat header", async () => {
    const page = await newPage(IPHONE);
    const target = await firstVisibleSession();
    expect(target).toBeDefined();
    if (target === undefined) return;
    await gotoAppMobile(page, `${target.agent}:${target.id}`);

    // Regression: the drawer used to render inside the sidebar Sheet and was
    // invisible on mobile — it now lives outside it.
    await page.locator('button[aria-label^="Session details:"]').first().click();
    const drawer = page.locator('[data-slot="drawer-popup"]');
    await drawer.waitFor({ state: "visible", timeout: 10_000 });
    expect(await drawer.innerText()).toContain("Session details");
    await page.close();
  });
});
