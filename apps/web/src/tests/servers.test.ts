import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { AuthError } from "../lib/api";
import {
  createServer,
  deleteServer,
  gatewayTarget,
  listServers,
  serverTarget,
  updateServer,
  type ManagedServer,
  type ServerInput,
} from "../lib/servers";

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

const calls: Array<{ url: string; init?: RequestInit }> = [];

const stubFetch = (handler: (url: string) => Response | Promise<Response>): void => {
  vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === "string" ? input : input instanceof Request ? input.url : input.toString();
    calls.push({ url, init });
    return handler(url);
  });
};

const SERVER: ManagedServer = {
  id: "srv_1",
  label: "box",
  host: "box.example",
  port: 8787,
  auth: null,
  ssh: null,
};

const INPUT: ServerInput = {
  label: "box",
  host: "box.example",
  port: 8787,
  auth: null,
  ssh: null,
};

beforeEach(() => {
  calls.length = 0;
  store.clear();
  vi.stubGlobal("localStorage", storage);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("server registry API", () => {
  it("listServers unwraps the envelope", async () => {
    stubFetch(() => Response.json({ servers: [SERVER] }));
    await expect(listServers()).resolves.toEqual([SERVER]);
    expect(calls[0]?.url).toBe("/api/servers");
  });

  it("createServer posts the input and returns the server", async () => {
    stubFetch(() => Response.json({ server: SERVER }));
    await expect(createServer(INPUT)).resolves.toEqual(SERVER);
    expect(calls[0]?.init?.method).toBe("POST");
    expect(JSON.parse(calls[0]?.init?.body as string)).toEqual(INPUT);
  });

  it("updateServer PATCHes /api/servers/:id", async () => {
    stubFetch(() => Response.json({ server: SERVER }));
    await expect(updateServer("srv 1", INPUT)).resolves.toEqual(SERVER);
    expect(calls[0]?.url).toBe("/api/servers/srv%201");
    expect(calls[0]?.init?.method).toBe("PATCH");
  });

  it("deleteServer resolves void", async () => {
    stubFetch(() => Response.json({ ok: true }));
    await expect(deleteServer("srv_1")).resolves.toBeUndefined();
    expect(calls[0]?.init?.method).toBe("DELETE");
  });

  it("sends the local token when one is stored", async () => {
    store.set("sepia:token", "tok");
    stubFetch(() => Response.json({ servers: [] }));
    await listServers();
    expect(calls[0]?.init?.headers).toMatchObject({ authorization: "Bearer tok" });
  });
});

describe("serversFetch error mapping", () => {
  it("prefers the server's error body when present", async () => {
    stubFetch(() => Response.json({ error: "host unreachable" }, { status: 400 }));
    await expect(listServers()).rejects.toThrow("host unreachable");
  });

  it.each([
    [400, "The server rejected that entry"],
    [404, "Unknown server"],
    [500, "The server hit an error — try again"],
    [418, "Request failed (418)"],
  ])("falls back to a friendly message for HTTP %i", async (code, message) => {
    stubFetch(() => new Response("plain text", { status: code }));
    await expect(listServers()).rejects.toThrow(message);
  });

  it("throws AuthError on 401", async () => {
    stubFetch(() => new Response("x", { status: 401 }));
    await expect(listServers()).rejects.toBeInstanceOf(AuthError);
  });

  it("reports unreachable servers", async () => {
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("fetch failed");
    });
    await expect(listServers()).rejects.toThrow("Can't reach the Sepia server");
  });
});

describe("serverTarget", () => {
  it("routes through the local node's proxy with a generous timeout", () => {
    store.set("sepia:token", "tok");
    expect(serverTarget(SERVER)).toEqual({
      baseUrl: "/api/servers/srv_1/proxy",
      token: "tok",
      timeoutMs: 12_000,
    });
  });
});

describe("gatewayTarget", () => {
  it("routes through the node's gateway mount with the node's own token", () => {
    store.set("sepia:token", "tok");
    expect(gatewayTarget("srv_1")).toEqual({
      baseUrl: "/api/gateway/srv_1",
      token: "tok",
      timeoutMs: 12_000,
    });
  });

  it("encodes the registry id", () => {
    expect(gatewayTarget("srv a/b").baseUrl).toBe("/api/gateway/srv%20a%2Fb");
  });
});
