import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import { apiSessions, gotoApp } from "./helpers/app.js";

describe("chat", () => {
  afterAll(closeBrowser);

  e2e("a sent message is optimistic — it shows before the POST resolves", async () => {
    const page = await newPage();
    // Stall the prompt POST so the optimistic row is observable.
    await page.route("**/api/sessions/*/prompt", async (route) => {
      await new Promise((r) => setTimeout(r, 2500));
      await route.continue();
    });
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    if (target === undefined) return;
    await gotoApp(page, `${target.agent}:${target.id}`);

    const marker = `e2e-optimistic-${Date.now()}`;
    const input = page.locator("main textarea, main [contenteditable]").first();
    await input.fill(marker);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(300); // well before the delayed POST lands
    const shown = await page.evaluate(
      (m) => document.querySelector("main")?.textContent?.includes(m) ?? false,
      marker,
    );
    expect(shown).toBe(true);
    await page.close();
  });
});
