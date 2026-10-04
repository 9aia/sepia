import { test } from "vite-plus/test";

export const WEB = process.env.SEPIA_E2E_WEB ?? "http://localhost:3000";
export const API = process.env.SEPIA_E2E_API ?? "http://127.0.0.1:8787";

const up = async (url: string): Promise<boolean> => {
  try {
    const res = await fetch(url, { signal: AbortSignal.timeout(3000) });
    return res.ok;
  } catch {
    return false;
  }
};

let stackChecked: Promise<boolean> | null = null;

/** Both legs reachable — the web dev server + the API it proxies. */
export const stackReady = (): Promise<boolean> => {
  stackChecked ??= (async () => (await up(`${API}/api/health`)) && (await up(WEB)))();
  return stackChecked;
};

/**
 * The e2e test entry: `e2e("name", async ({ page }) => ...)`.
 * Skips (not fails) when the stack isn't up — the suite is opt-in, so an
 * unreachable dev server shouldn't look broken. SEPIA_E2E_REQUIRED=1 turns a
 * skipped stack into a failure for CI that really wants it.
 */
export const e2e = test.extend<{ e2eOk: boolean }>({
  e2eOk: [
    async ({ skip }, use) => {
      const ok = await stackReady();
      if (!ok) {
        if (process.env.SEPIA_E2E_REQUIRED === "1") {
          throw new Error(`e2e stack unreachable (web ${WEB}, api ${API})`);
        }
        skip();
      }
      await use(ok);
    },
    { auto: true },
  ],
});
