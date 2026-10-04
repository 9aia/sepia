import type { Page } from "playwright";
import { API, WEB } from "./stack.js";

export interface SessionRow {
  readonly id: string;
  readonly agent: string;
  readonly title: string;
  readonly archived: boolean;
  readonly locked?: boolean;
  readonly busy?: boolean;
}

/** Sessions straight from the API — no page needed. */
export const apiSessions = async (): Promise<SessionRow[]> => {
  const res = await fetch(`${API}/api/sessions`, { signal: AbortSignal.timeout(10_000) });
  if (!res.ok) throw new Error(`sessions fetch failed: ${res.status}`);
  const body = (await res.json()) as { sessions: SessionRow[] };
  return body.sessions;
};

export const patchSession = async (
  session: Pick<SessionRow, "id" | "agent">,
  patch: Record<string, unknown>,
): Promise<void> => {
  const res = await fetch(`${API}/api/sessions/${session.id}?agent=${session.agent}`, {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(patch),
  });
  if (!res.ok) throw new Error(`PATCH ${session.id} failed: ${res.status}`);
};

/** Sidebar title text per section header, in render order. */
export const sectionTitles = async (page: Page): Promise<string[]> =>
  page.evaluate(() =>
    [...document.querySelectorAll('[data-slot="sidebar"] section')]
      .map((s) => s.querySelector("button")?.textContent?.trim() ?? "")
      .filter(Boolean),
  );

/** Titles inside a named sidebar section. */
export const sectionRows = async (page: Page, label: string): Promise<string[]> =>
  page.evaluate((wanted) => {
    const section = [...document.querySelectorAll('[data-slot="sidebar"] section')].find(
      (s) => s.querySelector("button")?.textContent?.includes(wanted) ?? false,
    );
    return [...(section?.querySelectorAll(".truncate") ?? [])]
      .map((r) => r.textContent?.trim() ?? "")
      .filter(Boolean);
  }, label);

/** Open the app, optionally deep-linked; waits for session rows to render. */
const waitForRows = async (page: Page): Promise<void> => {
  await page.waitForSelector(
    '[data-slot="sidebar"] .truncate, [data-slot="sidebar"] [data-slot="empty-title"]',
    { timeout: 30_000, state: "attached" },
  );
};

export const gotoApp = async (page: Page, session?: string): Promise<void> => {
  const url = session === undefined ? `${WEB}/` : `${WEB}/?session=${encodeURIComponent(session)}`;
  await page.goto(url, { waitUntil: "domcontentloaded" });
  // Rows render as .truncate spans; the empty state covers genuinely-empty
  // lists. One reload if the first load stalls — the dev server can flake
  // under several pages' worth of transforms at once.
  const boot = async (): Promise<void> => {
    await page.waitForSelector('[data-slot="sidebar"]', { timeout: 30_000 });
    await waitForRows(page);
  };
  try {
    await boot();
  } catch {
    await page.reload({ waitUntil: "domcontentloaded" });
    await boot();
  }
};

/** Click the sidebar row carrying this title. */
export const openSession = async (page: Page, title: string): Promise<void> => {
  const row = page
    .locator('[data-slot="sidebar"] .group\\/row button, [data-slot="sidebar"] .truncate')
    .filter({ hasText: title })
    .first();
  await row.click();
  await page.waitForTimeout(300);
};

/** Sessions safe to interact with — visible and not held by a live process. */
export const chatReadySessions = async (): Promise<SessionRow[]> =>
  (await apiSessions()).filter((s) => s.archived !== true && s.locked !== true && s.busy !== true);

/**
 * First chat-ready session whose stored history holds a user/assistant
 * message — the reply/copy footer actions only render on those rows.
 */
export const pickSessionWithHistory = async (): Promise<SessionRow | undefined> => {
  for (const session of await chatReadySessions()) {
    try {
      const res = await fetch(
        `${API}/api/sessions/${encodeURIComponent(session.id)}/history?agent=${session.agent}&limit=30`,
        { signal: AbortSignal.timeout(10_000) },
      );
      if (!res.ok) continue;
      const body = (await res.json()) as { messages: ReadonlyArray<{ role: string }> };
      if (body.messages.some((m) => m.role === "user" || m.role === "assistant")) return session;
    } catch {
      // Unreadable history — try the next session.
    }
  }
  return undefined;
};

/**
 * Mobile navigation: no persistent sidebar exists (it's a Sheet), so wait
 * for the chat header's trigger instead. A session must be selectable —
 * deep-link or rely on the app's auto-select of the first row.
 */
export const gotoAppMobile = async (page: Page, session?: string): Promise<void> => {
  const url = session === undefined ? `${WEB}/` : `${WEB}/?session=${encodeURIComponent(session)}`;
  await page.goto(url, { waitUntil: "domcontentloaded" });
  try {
    await page.waitForSelector('[data-slot="sidebar-trigger"]', { timeout: 30_000 });
  } catch {
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForSelector('[data-slot="sidebar-trigger"]', { timeout: 30_000 });
  }
};

/** Open the mobile sidebar Sheet and wait for rows to render inside it. */
export const openMobileSidebar = async (page: Page): Promise<void> => {
  await page.locator('[data-slot="sidebar-trigger"]').first().click();
  const sheet = page.locator('[data-slot="sidebar"][data-mobile="true"]');
  await sheet.waitFor({ state: "visible", timeout: 15_000 });
  await sheet
    .locator('.truncate, [data-slot="empty-title"]')
    .first()
    .waitFor({ state: "attached", timeout: 15_000 });
};

/** Open a session row's ⋯ dropdown and click a menu item by exact label. */
export const rowMenuAction = async (page: Page, title: string, item: string): Promise<void> => {
  const trigger = page.getByLabel(`Actions for session ${title}`, { exact: true }).first();
  // The trigger is hover-revealed — hover the row before clicking.
  await trigger.hover();
  await trigger.click();
  await page
    .locator('[data-slot="dropdown-menu-item"]')
    .filter({ hasText: new RegExp(`^${item.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`) })
    .first()
    .click();
};

/** Open Settings from the sidebar's account menu; waits for the section nav. */
export const openSettings = async (page: Page): Promise<void> => {
  await page.getByLabel("Account menu", { exact: true }).click();
  await page
    .locator('[data-slot="dropdown-menu-item"]')
    .filter({ hasText: /^Settings$/ })
    .first()
    .click();
  await page.waitForSelector('nav[aria-label="Settings sections"]', {
    state: "visible",
    timeout: 15_000,
  });
};
