import { describe, expect, it } from "vite-plus/test";
import { flattenHistory } from "../hooks/query/useHistory";
import type { InfiniteData } from "@tanstack/react-query";
import type { HistoryPage } from "../lib/types";

const page = (start: number, contents: string[]): HistoryPage => ({
  messages: contents.map((content, i) => ({
    role: "user",
    content,
    createdAt: start + i,
    toolName: undefined,
  })),
  total: contents.length + start,
  start,
});

describe("flattenHistory", () => {
  it("returns [] for undefined data", () => {
    expect(flattenHistory(undefined)).toEqual([]);
  });

  it("flattens newest-first pages oldest → newest", () => {
    const data: InfiniteData<HistoryPage> = {
      pages: [page(4, ["e", "f"]), page(0, ["a", "b", "c", "d"])],
      pageParams: [undefined, 4],
    };
    expect(flattenHistory(data).map((m) => m.content)).toEqual(["a", "b", "c", "d", "e", "f"]);
  });

  it("handles empty pages", () => {
    const data: InfiniteData<HistoryPage> = {
      pages: [page(0, [])],
      pageParams: [undefined],
    };
    expect(flattenHistory(data)).toEqual([]);
  });

  it("single page passes through in order", () => {
    const data: InfiniteData<HistoryPage> = {
      pages: [page(0, ["a", "b"])],
      pageParams: [undefined],
    };
    expect(flattenHistory(data).map((m) => m.content)).toEqual(["a", "b"]);
  });
});
