import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import { gotoApp, pickSessionWithHistory } from "./helpers/app.js";

const squash = (s: string): string => s.replace(/\s+/g, " ").trim();

describe("replying to a message", () => {
  afterAll(closeBrowser);

  e2e("reply quotes the message in the composer and sends it as a blockquote", async () => {
    const page = await newPage();
    // Stub the prompt POST — the optimistic row is the thing under test, and
    // a real send would spin up an agent run.
    await page.route("**/api/sessions/*/prompt", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: '{"ok":true}',
      });
    });
    const target = await pickSessionWithHistory();
    expect(target).toBeDefined();
    if (target === undefined) return;
    await gotoApp(page, `${target.agent}:${target.id}`);

    const row = page.locator('[data-slot="message"]').first();
    await row.waitFor({ state: "visible", timeout: 30_000 });
    const author = (await row.getAttribute("data-align")) === "end" ? "You" : "Assistant";
    const quoted = squash(await row.locator('[data-slot="bubble-content"]').innerText());
    expect(quoted.length).toBeGreaterThan(0);

    // Footer actions reveal on hover/focus.
    await row.hover();
    await row.locator('button[aria-label="Reply to message"]').click();

    // The composer shows the pending quote: author label + dismiss button.
    const dismiss = page.locator('button[aria-label="Dismiss reply"]');
    await dismiss.waitFor({ state: "visible", timeout: 10_000 });
    const preview = dismiss.locator("xpath=..");
    expect(squash(await preview.innerText())).toContain(author);

    const marker = `e2e-reply-${Date.now()}`;
    await page.locator("main textarea").first().fill(marker);
    await page.keyboard.press("Enter");

    // Another client may already hold the session (a stale attach from an
    // earlier run) — submitting then asks to take over; confirming sends the
    // held message once the attach resolves writable.
    const takeover = page.getByRole("button", { name: "Take over", exact: true });
    try {
      await takeover.waitFor({ state: "visible", timeout: 3000 });
      await takeover.click();
    } catch {
      // Writable attach — the message was sent directly.
    }

    // The optimistic user row embeds the quote as a markdown blockquote.
    const sent = page
      .locator('[data-slot="message"][data-align="end"]', { hasText: marker })
      .last();
    await sent.waitFor({ state: "visible", timeout: 10_000 });
    const quote = sent.locator("blockquote");
    await quote.waitFor({ state: "attached", timeout: 10_000 });
    expect(squash(await quote.innerText())).toContain(quoted.slice(0, 20));
    await page.close();
  });
});
