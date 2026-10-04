import type { Page } from "playwright";
import { API, WEB } from "./stack.js";

export interface SessionRow {
  readonly id: string;
  readonly agent: string;
  readonly title: string;
  readonly archived: boolean;
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
  await page.waitForSelector('[data-slot="sidebar"]', { timeout: 30_000 });
  // Rows render as .truncate spans; the empty state covers genuinely-empty
  // lists. One reload if the first load stalls — the dev server can flake
  // under several pages' worth of transforms at once.
  try {
    await waitForRows(page);
  } catch {
    await page.reload({ waitUntil: "domcontentloaded" });
    await waitForRows(page);
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
