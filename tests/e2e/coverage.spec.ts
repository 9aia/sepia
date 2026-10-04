import { execSync } from "node:child_process";
import { rmSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { API, WEB } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import { apiSessions, gotoApp, openSession, openSettings, patchSession } from "./helpers/app.js";

const post = async (path: string, body?: unknown): Promise<Response> =>
  fetch(`${API}${path}`, {
    method: "POST",
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  });

const del = async (path: string): Promise<Response> =>
  fetch(`${API}${path}`, { method: "DELETE", signal: AbortSignal.timeout(30_000) });

const CLINE_DIR = join(homedir(), ".cline", "data");

/** A converted session lands on disk + the cline index — remove both. */
const cleanupClineSession = (id: string): void => {
  rmSync(join(CLINE_DIR, "sessions", id), { recursive: true, force: true });
  try {
    execSync(
      `sqlite3 "${join(CLINE_DIR, "db", "sessions.db")}" "DELETE FROM sessions WHERE session_id='${id}'"`,
      { stdio: "ignore" },
    );
  } catch {
    // Index cleanup best-effort — the dir removal already unlists it.
  }
};

describe("session conversion", () => {
  afterAll(closeBrowser);

  e2e("POST /api/sessions/:id/convert rejects an unknown target agent", async () => {
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    expect(target).toBeDefined();
    if (target === undefined) return;

    const res = await post(
      `/api/sessions/${encodeURIComponent(target.id)}/convert?agent=${target.agent}`,
      { agent: "bogus" },
    );
    expect(res.status).toBe(400);
    await expect(res.json()).resolves.toEqual({ error: "agent must be 'cline' or 'devin'" });
  });

  e2e("converting a devin session produces a resumable cline session", async () => {
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.agent === "devin" && !s.archived && !s.locked);
    expect(target).toBeDefined();
    if (target === undefined) return;

    let newId: string | null = null;
    try {
      const res = await post(`/api/sessions/${encodeURIComponent(target.id)}/convert?agent=devin`, {
        agent: "cline",
      });
      expect(res.status).toBe(200);
      const body = (await res.json()) as { sessionId: string };
      newId = body.sessionId;
      expect(newId).not.toBe(target.id);

      // The converted copy is listed by the merged session list as a cline row.
      const after = await apiSessions();
      const converted = after.find((s) => s.id === newId);
      expect(converted).toBeDefined();
      expect(converted?.agent).toBe("cline");

      // And it renders in the sidebar alongside the original.
      const page = await newPage();
      try {
        await gotoApp(page);
        const text = await page.evaluate(
          () => document.querySelector('[data-slot="sidebar"]')?.textContent ?? "",
        );
        expect(text).toContain(target.title.slice(0, 20));
      } finally {
        await page.close();
      }
    } finally {
      if (newId !== null) cleanupClineSession(newId);
      const after = await apiSessions();
      expect(after.some((s) => s.id === newId)).toBe(false);
    }
  });
});

describe("cancel", () => {
  afterAll(closeBrowser);

  e2e("cancel on a live session returns ok; on a detached id it 400s", async () => {
    const sessions = await apiSessions();
    const detached = sessions.find((s) => !s.locked);
    expect(detached).toBeDefined();
    if (detached === undefined) return;

    // A stored-but-not-attached session has no live run to cancel.
    const notAttached = await post(
      `/api/sessions/${encodeURIComponent(detached.id)}/cancel?agent=${detached.agent}`,
    );
    expect(notAttached.status).toBe(400);

    // Create a real live session, cancel it, then clean it up.
    const created = await post("/api/sessions", {
      cwd: "/tmp",
      title: "e2e cancel probe (auto-deleted)",
    });
    expect(created.status).toBe(201);
    const { id } = (await created.json()) as { id: string };
    try {
      const cancelled = await post(`/api/sessions/${encodeURIComponent(id)}/cancel?agent=devin`);
      expect(cancelled.status).toBe(200);
      await expect(cancelled.json()).resolves.toEqual({ ok: true });
    } finally {
      await del(`/api/sessions/${encodeURIComponent(id)}?agent=devin`);
      const after = await apiSessions();
      expect(after.some((s) => s.id === id)).toBe(false);
    }
  });
});

describe("sidebar filter", () => {
  afterAll(closeBrowser);

  e2e("the filter input narrows the list and ?q= round-trips through the URL", async () => {
    const page = await newPage();
    await gotoApp(page);
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    const other = sessions.find(
      (s) => s !== target && s.archived !== true && s.title !== target?.title,
    );
    expect(target).toBeDefined();
    if (target === undefined || other === undefined) return;

    const sidebarText = async (): Promise<string> =>
      page.evaluate(() => document.querySelector('[data-slot="sidebar"]')?.textContent ?? "");

    try {
      // Open the search popover and type a distinctive slice of the title.
      await page.getByLabel("Search sessions", { exact: true }).click();
      const input = page.getByLabel("Filter sessions", { exact: true });
      await input.waitFor({ state: "visible", timeout: 10_000 });
      const needle = target.title.slice(0, 12);
      await input.fill(needle);
      await page.waitForTimeout(600); // 200ms debounce + refetch/render

      const url = new URL(page.url());
      expect(url.searchParams.get("q")).toBe(needle);
      expect(await sidebarText()).toContain(target.title.slice(0, 15));
      expect(await sidebarText()).not.toContain(other.title.slice(0, 15));

      // Escape closes the popover; the query stays in the URL until cleared.
      await input.press("Escape");
      await input.fill("");
      await page.waitForTimeout(600);
      expect(new URL(page.url()).searchParams.get("q")).toBeNull();
      expect(await sidebarText()).toContain(other.title.slice(0, 15));
    } finally {
      // The filter lives in the URL — make sure the page can't leak it.
      await page.goto(`${WEB}/`, { waitUntil: "domcontentloaded" }).catch(() => {});
    }
    await page.close();
  });
});

describe("keybinds", () => {
  afterAll(closeBrowser);

  e2e("Shift+/ opens settings on the Keyboard section; a rebind records and restores", async () => {
    const page = await newPage();
    await gotoApp(page);

    try {
      await page.keyboard.press("Shift+/");
      await page.waitForSelector('[data-slot="dialog-content"]', {
        state: "visible",
        timeout: 15_000,
      });
      // Opened straight on the Keyboard section (app.keybinds → "keyboard").
      const keyboardNav = page.locator('nav[aria-label="Settings sections"] button', {
        hasText: "Keyboard",
      });
      await keyboardNav.waitFor({ state: "visible", timeout: 10_000 });
      await keyboardNav.click();

      // Rebind "New session": Change → press P → the kbd shows P.
      const row = page.locator('[data-spy="keyboard"] .flex.items-center', {
        hasText: "New session",
      });
      await row.getByRole("button", { name: "Change" }).click();
      await page.keyboard.press("p");
      await page.waitForFunction(
        () => {
          const row = [
            ...document.querySelectorAll('[data-spy="keyboard"] .flex.items-center'),
          ].find((r) => r.textContent?.includes("New session"));
          return row?.querySelector("kbd")?.textContent === "P";
        },
        { timeout: 5_000 },
      );

      // Restore defaults brings the binding back and clears the override.
      await page.getByRole("button", { name: "Restore defaults" }).click();
      await page.waitForFunction(
        () => {
          const row = [
            ...document.querySelectorAll('[data-spy="keyboard"] .flex.items-center'),
          ].find((r) => r.textContent?.includes("New session"));
          return row?.querySelector("kbd")?.textContent === "N";
        },
        { timeout: 5_000 },
      );
    } finally {
      // Settings are localStorage-scoped to this incognito page — just close.
      await page.keyboard.press("Escape").catch(() => {});
    }
    await page.close();
  });

  e2e("a disabled binding reports Disabled and re-enables", async () => {
    const page = await newPage();
    await gotoApp(page);
    await openSettings(page);

    const keyboardNav = page.locator('nav[aria-label="Settings sections"] button', {
      hasText: "Keyboard",
    });
    await keyboardNav.waitFor({ state: "visible", timeout: 10_000 });
    await keyboardNav.click();

    const row = page.locator('[data-spy="keyboard"] .flex.items-center', {
      hasText: "Toggle sidebar",
    });
    await row.getByRole("button", { name: "Disable" }).click();
    await row.getByText("Disabled", { exact: true }).waitFor({ state: "visible", timeout: 5_000 });

    await row.getByRole("button", { name: "Enable" }).click();
    await page.waitForFunction(
      () => {
        const row = [...document.querySelectorAll('[data-spy="keyboard"] .flex.items-center')].find(
          (r) => r.textContent?.includes("Toggle sidebar"),
        );
        const keys = [...(row?.querySelectorAll("kbd") ?? [])].map((k) => k.textContent);
        return keys.join("+") === "Ctrl+B";
      },
      { timeout: 5_000 },
    );
    await page.close();
  });
});

describe("deep links", () => {
  afterAll(closeBrowser);

  e2e("a link to a nonexistent session lands on the empty state, not a crash", async () => {
    const page = await newPage();
    await gotoApp(page, "devin:no-such-session-e2e");

    // resolveSession can't find the id — the panel shows the EmptyScreen
    // instead of an attach error loop or a blank pane.
    const empty = page.getByText("No session selected", { exact: true });
    await empty.waitFor({ state: "visible", timeout: 15_000 });
    expect(await page.locator('[data-slot="sidebar"]').isVisible()).toBe(true);
    expect(new URL(page.url()).searchParams.get("session")).toBe("devin:no-such-session-e2e");
    await page.close();
  });

  e2e("a link to a real session still selects it", async () => {
    const page = await newPage();
    const sessions = await apiSessions();
    const target = sessions.find((s) => s.archived !== true);
    if (target === undefined) return;
    await gotoApp(page, `${target.agent}:${target.id}`);
    await openSession(page, target.title);
    expect(new URL(page.url()).searchParams.get("session")).toBe(`${target.agent}:${target.id}`);
    await page.close();
  });
});

describe("session meta via the UI", () => {
  afterAll(closeBrowser);

  e2e("pinning through PATCH keeps the row pinned after reload", async () => {
    const page = await newPage();
    await gotoApp(page);
    const sessions = await apiSessions();
    const target = sessions.find(
      (s) => s.archived !== true && (s as { pinned?: boolean }).pinned !== true,
    );
    expect(target).toBeDefined();
    if (target === undefined) return;

    try {
      await patchSession(target, { pinned: true });
      await page.reload({ waitUntil: "domcontentloaded" });
      await page.waitForSelector('[data-slot="sidebar"]');
      await page.waitForTimeout(1500);
      const titles = await page.evaluate(() =>
        [...document.querySelectorAll('[data-slot="sidebar"] section')]
          .map((s) => s.querySelector("button")?.textContent?.trim() ?? "")
          .filter(Boolean),
      );
      expect(titles.some((t) => t.toLowerCase().includes("pin"))).toBe(true);
    } finally {
      await patchSession(target, { pinned: false });
    }
    await page.close();
  });
});
