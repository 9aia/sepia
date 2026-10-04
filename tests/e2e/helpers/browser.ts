import {
  chromium,
  type Browser,
  type BrowserContext,
  type BrowserContextOptions,
  type Page,
} from "playwright";

let browser: Browser | null = null;

/** Shared browser for the whole file's context set — one launch per file. */
export const getBrowser = async (): Promise<Browser> => {
  browser ??= await chromium.launch({ headless: true });
  return browser;
};

export const closeBrowser = async (): Promise<void> => {
  await browser?.close();
  browser = null;
};

/** A clean incognito page — isolated storage, no cookies carried over. */
export const newPage = async (options?: BrowserContextOptions): Promise<Page> => {
  const b = await getBrowser();
  const ctx: BrowserContext = await b.newContext(options);
  return ctx.newPage();
};
