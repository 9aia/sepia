import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { QueryClient } from "@tanstack/react-query";
import {
  applyNodeEvent,
  historyKeyMatches,
  invalidationsForEvent,
  startNodeEventFeeds,
  subscribeNodeFeed,
} from "../lib/events";
import { nodesStore } from "../lib/nodes";
import { setToken } from "../lib/token";

const store = new Map<string, string>();

const storage = {
  getItem: (key: string) => store.get(key) ?? null,
  setItem: (key: string, value: string) => {
    store.set(key, String(value));
  },
  removeItem: (key: string) => {
    store.delete(key);
  },
};

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  readonly url: string;
  readonly listeners = new Map<string, (message: { data: string }) => void>();
  closed = false;

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(kind: string, listener: (message: { data: string }) => void): void {
    this.listeners.set(kind, listener);
  }

  emit(kind: string, data: unknown): void {
    this.listeners.get(kind)?.({ data: JSON.stringify(data) });
  }

  close(): void {
    this.closed = true;
  }
}

beforeEach(() => {
  store.clear();
  FakeEventSource.instances = [];
  vi.stubGlobal("localStorage", storage);
  vi.stubGlobal("EventSource", FakeEventSource);
  nodesStore.setState((prev) => ({ ...prev, peers: [] }));
});

afterEach(() => {
  nodesStore.setState((prev) => ({ ...prev, peers: [] }));
  vi.unstubAllGlobals();
});

describe("invalidationsForEvent", () => {
  it("maps each event kind to its query key", () => {
    expect(invalidationsForEvent({ kind: "session" })).toEqual([["sessions"]]);
    expect(invalidationsForEvent({ kind: "meta" })).toEqual([["sessions"]]);
    expect(invalidationsForEvent({ kind: "project" })).toEqual([["projects"]]);
    expect(invalidationsForEvent({ kind: "heartbeat" })).toEqual([]);
  });
});

describe("historyKeyMatches", () => {
  it("matches every history key form by the session-id tail", () => {
    expect(historyKeyMatches("s1", "s1")).toBe(true);
    expect(historyKeyMatches("devin:s1", "s1")).toBe(true);
    expect(historyKeyMatches("node_a:devin:s1", "s1")).toBe(true);
    expect(historyKeyMatches("local:cline:s1", "s1")).toBe(true);
  });

  it("rejects other sessions and suffix collisions", () => {
    expect(historyKeyMatches("devin:s11", "s1")).toBe(false);
    expect(historyKeyMatches("devin:as1", "s1")).toBe(false);
    expect(historyKeyMatches("s2", "s1")).toBe(false);
    expect(historyKeyMatches(42, "s1")).toBe(false);
    expect(historyKeyMatches(undefined, "s1")).toBe(false);
  });
});

describe("applyNodeEvent", () => {
  it("invalidates the matching query keys", () => {
    const client = new QueryClient();
    const spy = vi.spyOn(client, "invalidateQueries");

    applyNodeEvent(client, { kind: "session", id: "s1", agent: "devin", patch: { busy: true } });
    expect(spy).toHaveBeenCalledWith({ queryKey: ["sessions"] });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["history"], predicate: expect.any(Function) }),
    );

    applyNodeEvent(client, { kind: "meta", id: "s1", patch: { pinned: true } });
    expect(spy).toHaveBeenCalledWith({ queryKey: ["sessions"] });
    // meta events don't touch transcripts.
    expect(spy.mock.calls.filter(([arg]) => arg?.queryKey?.[0] === "history")).toHaveLength(1);

    applyNodeEvent(client, { kind: "project", id: "p1", patch: { name: "x" } });
    expect(spy).toHaveBeenCalledWith({ queryKey: ["projects"] });

    applyNodeEvent(client, { kind: "heartbeat" });
    expect(spy).toHaveBeenCalledTimes(4);
  });

  it("the history predicate only matches the event's session id", () => {
    const client = new QueryClient();
    const spy = vi.spyOn(client, "invalidateQueries");
    applyNodeEvent(client, { kind: "session", id: "s1" });

    const call = spy.mock.calls.find(([arg]) => arg?.predicate !== undefined);
    const predicate = call?.[0]?.predicate as
      | ((q: { queryKey: readonly unknown[] }) => boolean)
      | undefined;
    const q = (key: readonly unknown[]) => ({ queryKey: key });
    expect(predicate?.(q(["history", "devin:s1"]))).toBe(true);
    expect(predicate?.(q(["history", "node_x:cline:s1"]))).toBe(true);
    expect(predicate?.(q(["history", "devin:s2"]))).toBe(false);
  });
});

describe("subscribeNodeFeed", () => {
  it("opens /api/events on the target with the token as a query param", () => {
    subscribeNodeFeed({ baseUrl: "", token: "tok" }, () => {});
    expect(FakeEventSource.instances[0]?.url).toBe("/api/events?access_token=tok");

    subscribeNodeFeed({ baseUrl: "https://peer.example", token: null }, () => {});
    expect(FakeEventSource.instances[1]?.url).toBe("https://peer.example/api/events");
  });

  it("parses named frames into NodeEvents and closes cleanly", () => {
    const events: unknown[] = [];
    const close = subscribeNodeFeed({ baseUrl: "", token: null }, (e) => events.push(e));
    const source = FakeEventSource.instances[0];

    source?.emit("meta", { id: "s1", patch: { pinned: true } });
    source?.emit("session", { id: "s1", agent: "devin", patch: { busy: false } });
    source?.listeners.get("project")?.({ data: "not json" }); // ignored

    expect(events).toEqual([
      { kind: "meta", id: "s1", patch: { pinned: true } },
      { kind: "session", id: "s1", agent: "devin", patch: { busy: false } },
    ]);
    close();
    expect(source?.closed).toBe(true);
  });

  it("is a no-op without EventSource", () => {
    vi.stubGlobal("EventSource", undefined);
    expect(() => subscribeNodeFeed({ baseUrl: "", token: null }, () => {})()).not.toThrow();
  });
});

describe("startNodeEventFeeds", () => {
  const client = new QueryClient();

  it("opens a feed for the local node and each registered peer", () => {
    const stop = startNodeEventFeeds(client);
    expect(FakeEventSource.instances.map((s) => s.url)).toEqual(["/api/events"]);

    nodesStore.setState((prev) => ({
      ...prev,
      peers: [{ id: "node_peer", name: "peer", url: "https://peer.example", token: "ptok" }],
    }));
    expect(FakeEventSource.instances.map((s) => s.url)).toEqual([
      "/api/events",
      "https://peer.example/api/events?access_token=ptok",
    ]);

    nodesStore.setState((prev) => ({ ...prev, peers: [] }));
    expect(FakeEventSource.instances[1]?.closed).toBe(true);
    stop();
    expect(FakeEventSource.instances.every((s) => s.closed)).toBe(true);
  });

  it("re-subscribes the local feed when the token changes", () => {
    const stop = startNodeEventFeeds(client);
    expect(FakeEventSource.instances[0]?.url).toBe("/api/events");

    setToken("fresh");
    expect(FakeEventSource.instances[0]?.closed).toBe(true);
    expect(FakeEventSource.instances[1]?.url).toBe("/api/events?access_token=fresh");
    stop();
  });

  it("routes events to the query client", () => {
    const spy = vi.spyOn(client, "invalidateQueries");
    const stop = startNodeEventFeeds(client);
    FakeEventSource.instances[0]?.emit("project", { id: "p1", patch: { name: "x" } });
    expect(spy).toHaveBeenCalledWith({ queryKey: ["projects"] });
    stop();
  });
});
