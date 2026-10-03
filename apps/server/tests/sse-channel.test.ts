import { expect, test } from "vite-plus/test";
import { DEFAULT_KEEPALIVE_MS, keepAliveMsFromEnv, SseChannel } from "../src/sse-channel";

const drainStream = (channel: SseChannel): ReadableStream<Uint8Array> =>
  new ReadableStream<Uint8Array>(
    {
      start: (controller) => channel.start(controller),
      pull: () => channel.onPull(),
      cancel: () => {},
    },
    { highWaterMark: 1 },
  );

test("buffers frames pushed before start and flushes them", async () => {
  const channel = new SseChannel({ keepAliveMs: 0 });
  channel.push("data: one\n\n");
  channel.push("data: two\n\n");

  const reader = drainStream(channel).getReader();
  const first = new TextDecoder().decode((await reader.read()).value);
  const second = new TextDecoder().decode((await reader.read()).value);

  expect(first).toBe("data: one\n\n");
  expect(second).toBe("data: two\n\n");
});

test("flushes pending frames before closing when terminated before start", async () => {
  const channel = new SseChannel({ keepAliveMs: 0 });
  channel.push("data: done\n\n");
  channel.close();

  const reader = drainStream(channel).getReader();
  const frame = new TextDecoder().decode((await reader.read()).value);

  expect(frame).toBe("data: done\n\n");
  expect((await reader.read()).done).toBe(true);
});

test("errors the stream when the pending buffer overflows", async () => {
  const channel = new SseChannel({ keepAliveMs: 0, maxPending: 2 });
  channel.push("a");
  channel.push("b");
  channel.push("c");

  expect(channel.isTerminated).toBe(true);
  const reader = drainStream(channel).getReader();
  await expect(reader.read()).rejects.toThrow(/overflow/);
});

test("errors the stream when a client stops draining", async () => {
  const channel = new SseChannel({ keepAliveMs: 0, maxOutstandingBytes: 10 });
  const reader = drainStream(channel).getReader();
  channel.push("x".repeat(50));

  expect(channel.isTerminated).toBe(true);
  await expect(reader.read()).rejects.toThrow(/not draining/);
});

test("emits a keep-alive comment frame on the interval", async () => {
  const channel = new SseChannel({ keepAliveMs: 5 });
  const reader = drainStream(channel).getReader();

  const { value } = await reader.read();
  expect(new TextDecoder().decode(value)).toBe(": ping\n\n");
  channel.close();
});

test("keepAliveMsFromEnv parses and defaults", () => {
  expect(keepAliveMsFromEnv(undefined)).toBe(DEFAULT_KEEPALIVE_MS);
  expect(keepAliveMsFromEnv("")).toBe(DEFAULT_KEEPALIVE_MS);
  expect(keepAliveMsFromEnv("0")).toBe(0);
  expect(keepAliveMsFromEnv("250")).toBe(250);
  expect(keepAliveMsFromEnv("nope")).toBe(DEFAULT_KEEPALIVE_MS);
});
