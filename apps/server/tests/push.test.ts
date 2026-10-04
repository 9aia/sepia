import { beforeEach, describe, expect, it, vi } from "vite-plus/test";
import webpush from "web-push";
import { makePushStore, notifyForEvents, type PushSubscription } from "../src/push";
import type { MetaStore } from "../src/meta";

vi.mock("web-push", () => ({
  default: {
    generateVAPIDKeys: vi.fn(() => ({ publicKey: "pub-key", privateKey: "priv-key" })),
    setVapidDetails: vi.fn(),
    sendNotification: vi.fn(async () => ({})),
  },
}));

const generateVAPIDKeys = vi.mocked(webpush.generateVAPIDKeys);
const setVapidDetails = vi.mocked(webpush.setVapidDetails);
const sendNotification = vi.mocked(webpush.sendNotification);

/** Only config()/setConfig are used by the push store — a minimal fake suffices. */
const fakeMeta = (config: Record<string, unknown> = {}): MetaStore =>
  ({
    config: () => config,
    setConfig: (key: string, value: unknown) => {
      config[key] = value;
    },
  }) as unknown as MetaStore;

const sub = (endpoint: string, prefs?: PushSubscription["prefs"]): PushSubscription => ({
  endpoint,
  keys: { auth: "a", p256dh: "p" },
  prefs: prefs ?? { done: true, permission: true },
});

beforeEach(() => {
  vi.clearAllMocks();
  sendNotification.mockResolvedValue({} as never);
});

describe("makePushStore", () => {
  it("generates VAPID keys once and persists them in config", () => {
    const meta = fakeMeta();
    const store = makePushStore(meta);
    expect(store.publicKey).toBe("pub-key");
    expect(generateVAPIDKeys).toHaveBeenCalledTimes(1);
    expect(setVapidDetails).toHaveBeenCalledWith("mailto:sepia@localhost", "pub-key", "priv-key");

    // A second store over the same config reuses the persisted keys.
    const again = makePushStore(meta);
    expect(again.publicKey).toBe("pub-key");
    expect(generateVAPIDKeys).toHaveBeenCalledTimes(1);
  });

  it("upserts by endpoint and removes subscriptions", () => {
    const store = makePushStore(fakeMeta());
    store.upsert(sub("e1"));
    store.upsert(sub("e2"));
    expect(store.list().map((s) => s.endpoint)).toEqual(["e1", "e2"]);

    // Re-upserting an endpoint replaces rather than duplicates.
    store.upsert(sub("e1", { done: false, permission: true }));
    expect(store.list()).toHaveLength(2);
    expect(store.list().find((s) => s.endpoint === "e1")?.prefs.done).toBe(false);

    store.remove("e1");
    expect(store.list().map((s) => s.endpoint)).toEqual(["e2"]);
  });

  it("applies default prefs to stored subscriptions that lack them", () => {
    const meta = fakeMeta({
      pushSubscriptions: [{ endpoint: "e1", keys: { auth: "a", p256dh: "p" } }],
    });
    const store = makePushStore(meta);
    expect(store.list()[0]?.prefs).toEqual({ done: true, permission: true });
  });

  it("send() only notifies subscriptions opted into the kind", async () => {
    const store = makePushStore(fakeMeta());
    store.upsert(sub("wants-done", { done: true, permission: false }));
    store.upsert(sub("wants-perm", { done: false, permission: true }));

    const sent = await store.send("done", "Title", "Body", "/?session=x");
    expect(sent).toBe(1);
    expect(sendNotification).toHaveBeenCalledTimes(1);
    const [target, payload, options] = sendNotification.mock.calls[0] as unknown as [
      { endpoint: string },
      string,
      { TTL: number },
    ];
    expect(target.endpoint).toBe("wants-done");
    expect(JSON.parse(payload)).toMatchObject({ title: "Title", body: "Body", url: "/?session=x" });
    expect(options.TTL).toBe(300);
  });

  it("send() prunes expired subscriptions (404/410) and keeps the rest", async () => {
    const store = makePushStore(fakeMeta());
    store.upsert(sub("expired"));
    store.upsert(sub("flaky"));
    store.upsert(sub("healthy"));
    sendNotification
      .mockRejectedValueOnce({ statusCode: 410 })
      .mockRejectedValueOnce({ statusCode: 500 })
      .mockResolvedValueOnce({} as never);

    const sent = await store.send("done", "T", "B", "/");
    expect(sent).toBe(1);
    expect(store.list().map((s) => s.endpoint)).toEqual(["flaky", "healthy"]);
  });
});

describe("notifyForEvents", () => {
  const storeWithSub = () => {
    const store = makePushStore(fakeMeta());
    store.upsert(sub("e1"));
    return store;
  };

  it("notifies done on RUN_FINISHED or RUN_ERROR", () => {
    const store = storeWithSub();
    notifyForEvents(store, "s1", "devin", "My session", [{ type: "RUN_FINISHED" }]);
    expect(sendNotification).toHaveBeenCalledTimes(1);
    const payload = JSON.parse(
      (sendNotification.mock.calls[0] as unknown as [unknown, string])[1],
    ) as Record<string, unknown>;
    expect(payload.title).toBe("Session finished");
    expect(payload.url).toBe("/?session=devin%3As1");
    expect(payload.tag).toBe("sepia-done");

    sendNotification.mockClear();
    notifyForEvents(store, "s1", undefined, "My session", [{ type: "RUN_ERROR" }]);
    const payload2 = JSON.parse(
      (sendNotification.mock.calls[0] as unknown as [unknown, string])[1],
    ) as Record<string, unknown>;
    expect(payload2.url).toBe("/?session=s1");
  });

  it("notifies permission on an acp:permission_request custom event", () => {
    const store = storeWithSub();
    notifyForEvents(store, "s1", undefined, "My session", [
      { type: "CUSTOM", name: "acp:permission_request" },
    ]);
    const payload = JSON.parse(
      (sendNotification.mock.calls[0] as unknown as [unknown, string])[1],
    ) as Record<string, unknown>;
    expect(payload.title).toBe("Approval needed");
    expect(payload.tag).toBe("sepia-permission");
  });

  it("ignores unrelated events", () => {
    const store = storeWithSub();
    notifyForEvents(store, "s1", undefined, "T", [
      { type: "TEXT_MESSAGE_CONTENT" },
      { type: "CUSTOM", name: "other" },
      {},
    ]);
    expect(sendNotification).not.toHaveBeenCalled();
  });

  it("fires both notifications for a run that ends awaiting permission", () => {
    const store = storeWithSub();
    notifyForEvents(store, "s1", undefined, "T", [
      { type: "CUSTOM", name: "acp:permission_request" },
      { type: "RUN_FINISHED" },
    ]);
    expect(sendNotification).toHaveBeenCalledTimes(2);
  });
});
