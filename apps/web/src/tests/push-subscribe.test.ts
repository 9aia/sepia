import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  isPushSupported,
  pushState,
  subscribePush,
  unsubscribePush,
  updatePushPrefs,
} from "../lib/push";
import { setToken } from "../lib/api";

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

/** Stub the trio isPushSupported checks: navigator.serviceWorker, window.PushManager, Notification. */
const installPushEnv = (options: {
  permission?: string;
  registration?: { pushManager: Record<string, unknown> };
}) => {
  vi.stubGlobal("navigator", {
    serviceWorker: {
      getRegistration: async () => options.registration,
      ready: Promise.resolve(options.registration),
    },
  });
  vi.stubGlobal("window", { PushManager: class {}, Notification: class {} });
  vi.stubGlobal("Notification", { permission: options.permission ?? "default" });
};

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", storage);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("isPushSupported", () => {
  it("is false without service worker / push / notification support", () => {
    vi.stubGlobal("navigator", {});
    vi.stubGlobal("window", {});
    expect(isPushSupported()).toBe(false);
  });

  it("is true when all three APIs exist", () => {
    installPushEnv({});
    expect(isPushSupported()).toBe(true);
  });
});

describe("pushState", () => {
  it("reports unsupported when the APIs are missing", async () => {
    vi.stubGlobal("navigator", {});
    vi.stubGlobal("window", {});
    await expect(pushState()).resolves.toBe("unsupported");
  });

  it("mirrors Notification.permission when not granted", async () => {
    installPushEnv({ permission: "denied" });
    await expect(pushState()).resolves.toBe("denied");
    installPushEnv({ permission: "default" });
    await expect(pushState()).resolves.toBe("default");
  });

  it("distinguishes granted from subscribed via the active subscription", async () => {
    installPushEnv({
      permission: "granted",
      registration: { pushManager: { getSubscription: async () => null } },
    });
    await expect(pushState()).resolves.toBe("granted");

    installPushEnv({
      permission: "granted",
      registration: { pushManager: { getSubscription: async () => ({ endpoint: "e" }) } },
    });
    await expect(pushState()).resolves.toBe("subscribed");
  });

  it("a missing registration still reports granted", async () => {
    installPushEnv({ permission: "granted", registration: undefined });
    await expect(pushState()).resolves.toBe("granted");
  });
});

describe("subscribePush", () => {
  const key = Buffer.from("public-key").toString("base64url");

  it("returns false without push support", async () => {
    vi.stubGlobal("navigator", {});
    vi.stubGlobal("window", {});
    await expect(subscribePush({ done: true, permission: true })).resolves.toBe(false);
  });

  it("returns false when the VAPID fetch fails", async () => {
    installPushEnv({ permission: "granted" });
    vi.stubGlobal("fetch", async () => new Response("x", { status: 500 }));
    await expect(subscribePush({ done: true, permission: true })).resolves.toBe(false);
  });

  it("subscribes with the VAPID key and posts the subscription with prefs", async () => {
    const subscribe = vi.fn(async () => ({
      endpoint: "https://push.example/sub",
      toJSON: () => ({ endpoint: "https://push.example/sub", keys: { auth: "a", p256dh: "p" } }),
    }));
    installPushEnv({ permission: "granted", registration: { pushManager: { subscribe } } });
    setToken("tok");

    const fetched: Array<{ url: string; init?: RequestInit }> = [];
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      fetched.push({ url, init });
      if (url === "/api/push/vapid") return Response.json({ publicKey: key });
      return Response.json({ ok: true });
    });

    await expect(subscribePush({ done: true, permission: false })).resolves.toBe(true);

    expect(subscribe).toHaveBeenCalledWith({
      userVisibleOnly: true,
      applicationServerKey: expect.any(Uint8Array),
    });
    const post = fetched.find((f) => f.url === "/api/push/subscribe");
    expect(post?.init?.method).toBe("POST");
    expect(post?.init?.headers).toMatchObject({ authorization: "Bearer tok" });
    const body = JSON.parse(post?.init?.body as string) as Record<string, unknown>;
    expect(body.endpoint).toBe("https://push.example/sub");
    expect(body.prefs).toEqual({ done: true, permission: false });
  });

  it("returns false when the registration POST fails", async () => {
    installPushEnv({
      permission: "granted",
      registration: {
        pushManager: { subscribe: async () => ({ endpoint: "e", toJSON: () => ({}) }) },
      },
    });
    vi.stubGlobal("fetch", async (url: string) =>
      url === "/api/push/vapid"
        ? Response.json({ publicKey: key })
        : new Response("x", { status: 500 }),
    );
    await expect(subscribePush({ done: true, permission: true })).resolves.toBe(false);
  });

  it("updatePushPrefs re-runs the subscribe flow", async () => {
    installPushEnv({ permission: "granted" });
    vi.stubGlobal("fetch", async () => new Response("x", { status: 500 }));
    await expect(updatePushPrefs({ done: false, permission: false })).resolves.toBe(false);
  });
});

describe("unsubscribePush", () => {
  it("no-ops without push support", async () => {
    vi.stubGlobal("navigator", {});
    vi.stubGlobal("window", {});
    await expect(unsubscribePush()).resolves.toBeUndefined();
  });

  it("no-ops when there is no active subscription", async () => {
    installPushEnv({
      permission: "granted",
      registration: { pushManager: { getSubscription: async () => null } },
    });
    const fetchSpy = vi.fn(async () => Response.json({}));
    vi.stubGlobal("fetch", fetchSpy);
    await unsubscribePush();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("DELETEs the endpoint and unsubscribes locally", async () => {
    const unsubscribe = vi.fn(async () => true);
    installPushEnv({
      permission: "granted",
      registration: {
        pushManager: {
          getSubscription: async () => ({ endpoint: "https://push.example/sub", unsubscribe }),
        },
      },
    });
    const fetched: Array<{ url: string; init?: RequestInit }> = [];
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      fetched.push({ url, init });
      return Response.json({ ok: true });
    });

    await unsubscribePush();
    expect(fetched[0]?.url).toBe("/api/push/subscribe");
    expect(fetched[0]?.init?.method).toBe("DELETE");
    expect(JSON.parse(fetched[0]?.init?.body as string)).toEqual({
      endpoint: "https://push.example/sub",
    });
    expect(unsubscribe).toHaveBeenCalled();
  });

  it("still unsubscribes locally when the DELETE fails", async () => {
    const unsubscribe = vi.fn(async () => true);
    installPushEnv({
      permission: "granted",
      registration: {
        pushManager: {
          getSubscription: async () => ({ endpoint: "e", unsubscribe }),
        },
      },
    });
    vi.stubGlobal("fetch", async () => {
      throw new Error("offline");
    });
    await unsubscribePush();
    expect(unsubscribe).toHaveBeenCalled();
  });
});
