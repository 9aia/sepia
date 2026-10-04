import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import { apiSessions, gotoApp, openSession, sectionTitles } from "./helpers/app.js";

describe("session list + selection", () => {
  afterAll(closeBrowser);

  e2e("the sidebar lists sessions under its sections", async () => {
    const page = await newPage();
    await gotoApp(page);
    const titles = await sectionTitles(page);
    expect(titles.length).toBeGreaterThan(0);
    await page.close();
  });

  e2e("clicking a session sets ?session= in the URL", async () => {
    const page = await newPage();
    await gotoApp(page);
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    expect(target).toBeDefined();
    if (target === undefined) return;
    await openSession(page, target.title);
    await page.waitForTimeout(500);
    expect(new URL(page.url()).searchParams.get("session")).toBe(`${target.agent}:${target.id}`);
    await page.close();
  });

  e2e("a deep link loads that session directly", async () => {
    const page = await newPage();
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    if (target === undefined) return;
    await gotoApp(page, `${target.agent}:${target.id}`);
    expect(new URL(page.url()).searchParams.get("session")).toBe(`${target.agent}:${target.id}`);
    const header = await page.evaluate(() => document.querySelector("header")?.textContent ?? "");
    expect(header).toContain(target.title.slice(0, 20));
    await page.close();
  });
});
