/**
 * Minimal hook renderer for hook-level tests — no testing-library needed:
 * react-dom + act + happy-dom. `result.current` holds the last render's
 * return value; `waitFor` spins until a predicate holds or the timeout hits.
 */
import { vi } from "vite-plus/test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export const makeQueryClient = (): QueryClient =>
  new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
      mutations: { retry: false },
    },
  });

export interface RenderedHook<T> {
  readonly result: { current: T | undefined };
  readonly queryClient: QueryClient;
  readonly rerender: () => void;
  readonly unmount: () => void;
}

export const renderHook = <T,>(hook: () => T, queryClient = makeQueryClient()): RenderedHook<T> => {
  const result: { current: T | undefined } = { current: undefined };
  const Probe = (): null => {
    result.current = hook();
    return null;
  };
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const tree = (
    <QueryClientProvider client={queryClient}>
      <Probe />
    </QueryClientProvider>
  );
  act(() => {
    root.render(tree);
  });
  return {
    result,
    queryClient,
    rerender: () =>
      act(() => {
        root.render(tree);
      }),
    unmount: () => {
      act(() => root.unmount());
      container.remove();
    },
  };
};

/** Poll until `fn()` is truthy — the async query/mutation settle helper. */
export const waitFor = async (fn: () => boolean, timeoutMs = 2_000): Promise<void> => {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    let ok = false;
    await act(async () => {
      ok = fn();
      await Promise.resolve();
    });
    if (ok) return;
    if (Date.now() > deadline) throw new Error("waitFor timed out");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
};

/** A fetch stub: path-matched canned responses, otherwise 404 JSON. */
export const stubFetch = (
  routes: ReadonlyArray<{
    readonly match: RegExp | string;
    readonly method?: string;
    readonly status?: number;
    readonly body?: unknown;
    readonly headers?: Record<string, string>;
  }>,
): { calls: Array<{ url: string; init?: RequestInit }> } => {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const impl = async (input: unknown, init?: RequestInit): Promise<Response> => {
    const url = typeof input === "string" ? input : (input as Request).url;
    calls.push({ url, init });
    const route = routes.find((r) => {
      const hit = typeof r.match === "string" ? url.includes(r.match) : r.match.test(url);
      return hit && (r.method === undefined || r.method === (init?.method ?? "GET"));
    });
    if (route === undefined) {
      return new Response(JSON.stringify({ error: "no stub" }), { status: 404 });
    }
    return new Response(route.body === undefined ? "null" : JSON.stringify(route.body), {
      status: route.status ?? 200,
      headers: { "content-type": "application/json", ...route.headers },
    });
  };
  vi.stubGlobal("fetch", impl);
  return { calls };
};
