import { expect, test } from "vite-plus/test";
import { EventType } from "@ag-ui/core";
import { encodeSse, sseHeaders } from "../src/sse.js";

test("encodes one SSE data frame per event", () => {
  const events = [
    { type: EventType.RUN_STARTED, threadId: "t", runId: "r" },
    { type: EventType.CUSTOM, name: "a", value: 1 },
  ] as const;

  expect(encodeSse(events)).toBe(
    `data: ${JSON.stringify(events[0])}\n\n` + `data: ${JSON.stringify(events[1])}\n\n`,
  );
});

test("terminates every frame with a blank line", () => {
  const event = { type: EventType.CUSTOM, name: "x", value: { ok: true } } as const;
  const output = encodeSse([event]);

  expect(output.endsWith("\n\n")).toBe(true);
  expect(output.split("\n\n").filter((part) => part.length > 0)).toEqual([
    `data: ${JSON.stringify(event)}`,
  ]);
});

test("exposes the SSE headers", () => {
  expect(sseHeaders).toEqual({
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache",
    Connection: "keep-alive",
  });
});
