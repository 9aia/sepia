import { afterAll, describe, expect } from "vite-plus/test";
import { e2e } from "./helpers/stack.js";
import { closeBrowser, newPage } from "./helpers/browser.js";
import { gotoApp, pickSessionWithHistory } from "./helpers/app.js";

const squash = (s: string): string => s.replace(/\s+/g, " ").trim();

/** Letters/digits only — a raw markdown line and its rendered form normalize equal. */
const alnum = (s: string): string =>
  s
    .replace(/[^\p{L}\p{N} ]+/gu, " ")
    .replace(/\s+/g, " ")
    .trim();

/** The attach POST settles readOnly/writable — prompts 400 before it. */
const waitForAttach = (page: Parameters<typeof gotoApp>[0]) =>
  page.waitForResponse(
    (res) => res.url().includes("/attach") && res.request().method() === "POST" && res.ok(),
    { timeout: 60_000 },
  );

describe("replying to a message", () => {
  afterAll(closeBrowser);

  e2e("reply quotes the message in the composer and sends it as a blockquote", async () => {
    const page = await newPage();
    // Stub the prompt POST — the optimistic row is the thing under test, and
    // a real send would deliver the prompt to a live agent. Trailing `*`
    // covers the ?agent= query, which Playwright includes in glob matching.
    // The body is captured: it's the ground truth for what got quoted —
    // DOM-derived expectations race the virtualized row window (below).
    let sentPrompt = "";
    await page.route("**/api/sessions/*/prompt*", async (route) => {
      const body = route.request().postDataJSON() as { text?: string };
      sentPrompt = typeof body.text === "string" ? body.text : "";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: '{"ok":true}',
      });
    });
    const target = await pickSessionWithHistory();
    expect(target).toBeDefined();
    if (target === undefined) return;
    const attach = waitForAttach(page);
    await gotoApp(page, `${target.agent}:${target.id}`);
    const attachRes = await attach;
    const readOnly = ((await attachRes.json()) as { readOnly?: boolean }).readOnly === true;

    // Some turns render a message row with an empty surface — pick one that
    // actually carries text to quote.
    const row = page
      .locator('[data-slot="message"]', {
        has: page.locator('[data-slot="bubble-content"]', { hasText: /\S/ }),
      })
      .first();
    await row.waitFor({ state: "visible", timeout: 30_000 });

    // Footer actions reveal on hover/focus. The mounted window can still be
    // shifting (virtualizer relayout, history pagination), so the row
    // `.first()` resolves to at click time may differ from the one seen at
    // hover — fine: the expectation is read from the preview + wire payload,
    // which always reflect the message actually quoted.
    await row.hover();
    await row.locator('button[aria-label="Reply to message"]').click();

    // The composer shows the pending quote: author label + dismiss button.
    const dismiss = page.locator('button[aria-label="Dismiss reply"]');
    await dismiss.waitFor({ state: "visible", timeout: 10_000 });
    const preview = dismiss.locator("xpath=..");
    const author = squash(await preview.locator("xpath=./div/div[1]").innerText());
    expect(["You", "Assistant"]).toContain(author);
    // textContent — innerText honors the line-clamp and can drop the tail.
    expect(squash((await preview.locator("p").textContent()) ?? "")).not.toBe("");

    const marker = `e2e-reply-${Date.now()}`;
    await page.locator("main textarea").first().fill(marker);
    await page.keyboard.press("Enter");

    if (readOnly) {
      // Held by another process (e.g. a stale attach from an earlier run) —
      // submitting asks to take over; confirming re-attaches writable and the
      // held message sends automatically.
      const reattach = waitForAttach(page);
      await page.getByRole("button", { name: "Take over", exact: true }).click();
      await reattach;
    }

    // The optimistic user row embeds the quote as a markdown blockquote. The
    // row transitions between the optimistic live row and flushed history —
    // the selector matches either, and the waitFor outlasts the handoff.
    const sent = page
      .locator('[data-slot="message"][data-align="end"]', { hasText: marker })
      .last();
    await sent.waitFor({ state: "visible", timeout: 15_000 });
    await expect.poll(() => sentPrompt, { timeout: 10_000 }).not.toBe("");

    const quote = sent.locator("blockquote");
    await quote.waitFor({ state: "attached", timeout: 10_000 });
    // "> " lines of the wire prompt are the quoted message — "> — Author" is
    // the attribution formatReplyPrompt appends. Match the first word of the
    // first wordy quote line: markdown rendering always preserves it, while
    // longer spans can split around link syntax.
    const quoted =
      sentPrompt
        .split("\n")
        .map((line) =>
          line.startsWith("> ") && !line.startsWith("> —") ? alnum(line.slice(2)) : "",
        )
        .find((text) => text !== "")
        ?.split(" ")[0] ?? "";
    expect(quoted).not.toBe("");
    const quoteText = await quote.innerText();
    expect(alnum(quoteText).split(" ")).toContain(quoted);
    expect(squash(quoteText)).toContain(`— ${author}`);
    await page.close();
  });
});
