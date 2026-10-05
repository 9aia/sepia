import { describe, expect, it } from "vite-plus/test";
import { keepAliveMsFromEnv, SseChannel } from "../src/sse-channel";

const drainStream = (channel: SseChannel): ReadableStream<Uint8Array> =>
  new ReadableStream<Uint8Array>(
    {
      start: (controller) => channel.start(controller),
      pull: () => channel.onPull(),
      cancel: () => {},
    },
    { highWaterMark: 1 },
  );

describe("SseChannel edge paths", () => {
  it("drops frames pushed after termination", async () => {
    const channel = new SseChannel({ keepAliveMs: 0 });
    const reader = drainStream(channel).getReader();
    channel.push("data: live\n\n");
    channel.close();
    channel.push("data: zombie\n\n");

    const frame = new TextDecoder().decode((await reader.read()).value);
    expect(frame).toBe("data: live\n\n");
    expect((await reader.read()).done).toBe(true);
  });

  it("onPull resets the outstanding-byte budget", async () => {
    const channel = new SseChannel({ keepAliveMs: 0, maxOutstandingBytes: 10 });
    drainStream(channel).getReader();
    channel.push("x".repeat(9));
    expect(channel.isTerminated).toBe(false);
    channel.onPull();
    channel.push("y".repeat(9));
    expect(channel.isTerminated).toBe(false);
    channel.push("z".repeat(20));
    expect(channel.isTerminated).toBe(true);
  });

  it("invokes onTerminate exactly once, with the reason", async () => {
    const reasons: Array<string | null> = [];
    const channel = new SseChannel({
      keepAliveMs: 0,
      maxPending: 1,
      onTerminate: (reason) => reasons.push(reason),
    });
    channel.push("a");
    channel.push("b"); // overflows the pending buffer
    channel.close(); // already terminated — no second callback
    expect(reasons).toEqual(["pending frame buffer overflow"]);
    expect(channel.isTerminated).toBe(true);
  });

  it("invokes onTerminate with null on a clean close", () => {
    const reasons: Array<string | null> = [];
    const channel = new SseChannel({
      keepAliveMs: 0,
      onTerminate: (reason) => reasons.push(reason),
    });
    channel.close();
    expect(reasons).toEqual([null]);
  });

  it("a reason-terminated channel errors the stream on late start", async () => {
    const channel = new SseChannel({ keepAliveMs: 0, maxPending: 0 });
    channel.push("overflow");
    const reader = drainStream(channel).getReader();
    await expect(reader.read()).rejects.toThrow(/overflow/);
  });

  it("stops the keep-alive timer once closed", async () => {
    const channel = new SseChannel({ keepAliveMs: 5 });
    const reader = drainStream(channel).getReader();
    channel.close();
    expect((await reader.read()).done).toBe(true);
  });
});

describe("keepAliveMsFromEnv", () => {
  it("floors fractional values and rejects negatives", () => {
    expect(keepAliveMsFromEnv("15.9")).toBe(15);
    expect(keepAliveMsFromEnv("-5")).toBe(15_000);
    expect(keepAliveMsFromEnv("Infinity")).toBe(15_000);
  });
});

it("enqueue on a dead controller terminates with 'stream already closed'", async () => {
  let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c;
    },
  });
  const reasons: Array<string | null> = [];
  const channel = new SseChannel({ keepAliveMs: 0, onTerminate: (r) => reasons.push(r) });
  channel.start(controller!);
  await stream.cancel();
  // subsequent pushes hit a dead controller — the channel terminates
  channel.push("event: x\n\n");
  expect(channel.isTerminated).toBe(true);
});

it("a terminated channel flushes pending frames without throwing on a dead controller", async () => {
  let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c;
    },
  });
  const channel = new SseChannel({ keepAliveMs: 0 });
  channel.push("event: one\ndata: {}\n\n");
  channel.close();
  // cancel the stream so the late start() flush hits a dead controller
  await stream.cancel();
  channel.start(controller!);
  expect(channel.isTerminated).toBe(true);
});

it("a keep-alive ping on a dead stream terminates the channel", async () => {
  let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c;
    },
  });
  const reasons: Array<string | null> = [];
  const channel = new SseChannel({ keepAliveMs: 1, onTerminate: (r) => reasons.push(r) });
  channel.start(controller!);
  await stream.cancel();
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(channel.isTerminated).toBe(true);
  expect(reasons).toContain("stream already closed");
});
