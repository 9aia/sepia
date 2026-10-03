import type { Event } from "@ag-ui/core";

export function encodeSse(events: ReadonlyArray<Event>): string {
  return events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("");
}

export const sseHeaders: Record<string, string> = {
  "Content-Type": "text/event-stream",
  "Cache-Control": "no-cache",
  Connection: "keep-alive",
};
