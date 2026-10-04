import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import { apiSessions, gotoApp, patchSession, rowMenuAction } from "./helpers/app.js";

describe("toasts", () => {
  afterAll(closeBrowser);

  e2e("archiving a session through the row menu shows a success toast", async () => {
    const page = await newPage();
    await gotoApp(page);
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    expect(target).toBeDefined();
    if (target === undefined) return;

    try {
      await rowMenuAction(page, target.title, "Archive");
      const toast = page.locator("[data-sonner-toast]", { hasText: "Session archived" });
      await toast.waitFor({ state: "visible", timeout: 15_000 });
    } finally {
      await patchSession(target, { archived: false });
    }
    await page.close();
  });

  e2e("a failed session update shows an error toast and the app keeps working", async () => {
    const page = await newPage();
    // Fail every PATCH — the archive below never reaches the server, so the
    // session stays put and the mutation's error path fires.
    await page.route("**/api/sessions/*", async (route) => {
      if (route.request().method() === "PATCH") {
        await route.fulfill({
          status: 500,
          contentType: "application/json",
          body: '{"error":"e2e forced failure"}',
        });
        return;
      }
      await route.continue();
    });
    await gotoApp(page);
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    expect(target).toBeDefined();
    if (target === undefined) return;

    try {
      await rowMenuAction(page, target.title, "Archive");
      const toast = page.locator("[data-sonner-toast]", {
        hasText: "Couldn't update the session",
      });
      await toast.waitFor({ state: "visible", timeout: 15_000 });

      // No crash — the sidebar still renders, and the server-side state is
      // untouched because the PATCH was intercepted.
      expect(await page.locator('[data-slot="sidebar"]').isVisible()).toBe(true);
      const after = await apiSessions();
      expect(after.find((s) => s.id === target.id)?.archived).not.toBe(true);
    } finally {
      await page.unroute("**/api/sessions/*");
      await patchSession(target, { archived: false });
    }
    await page.close();
  });
});
