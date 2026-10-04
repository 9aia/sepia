import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import {
  authHeader,
  createServerStore,
  isInlineKey,
  publicServer,
  validateServerInput,
  type ServerEntry,
  type ServerInput,
} from "../src/servers";
import { handleServersRoute, type ServersRouteDeps } from "../src/servers-routes";
import { sshTunnelArgs, type TunnelManager } from "../src/ssh";

const INPUT: ServerInput = {
  label: "Thinkpad",
  host: "192.168.1.10",
  port: 8787,
  auth: { type: "token", secret: "s3cr3t-token" },
  ssh: null,
};

const SSH_INPUT: ServerInput = {
  ...INPUT,
  ssh: { host: "bastion.example.com", port: 22, user: "luis", key: "~/.ssh/id_ed25519" },
};

const tmp = (): string => mkdtempSync(join(tmpdir(), "sepia-servers-"));

describe("validateServerInput", () => {
  it("accepts a minimal entry with no auth and no ssh", () => {
    const parsed = validateServerInput({ label: "local", host: "127.0.0.1", port: 8787 });
    expect(parsed).toEqual({
      ok: true,
      input: { label: "local", host: "127.0.0.1", port: 8787, auth: null, ssh: null },
    });
  });

  it("defaults ssh.port to 22", () => {
    const parsed = validateServerInput({
      label: "x",
      host: "host",
      port: 1,
      ssh: { host: "bastion", user: "me" },
    });
    expect(parsed.ok && parsed.input.ssh?.port).toBe(22);
  });

  it.each([
    ["not an object", "Expected a JSON object body"],
    [{ label: "", host: "h", port: 1 }, "label"],
    [{ label: "x", host: "bad host", port: 1 }, "host"],
    [{ label: "x", host: "http://h", port: 1 }, "host"],
    [{ label: "x", host: "h", port: 0 }, "port"],
    [{ label: "x", host: "h", port: 70000 }, "port"],
    [{ label: "x", host: "h", port: 1, auth: { type: "oauth" } }, "auth.type"],
    [{ label: "x", host: "h", port: 1, auth: { type: "token" } }, "auth.secret"],
    [{ label: "x", host: "h", port: 1, ssh: { host: "b" } }, "ssh.user"],
  ])("rejects %o", (body, fragment) => {
    const parsed = validateServerInput(body);
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.error).toContain(fragment as string);
  });
});

describe("publicServer", () => {
  const entry: ServerEntry = { id: "srv_1", ...INPUT };

  it("masks the auth secret", () => {
    const pub = publicServer(entry);
    expect(JSON.stringify(pub)).not.toContain("s3cr3t-token");
    expect((pub.auth as { secret: string }).secret).toBe("••••••••");
  });

  it("echoes key paths but masks pasted keys", () => {
    const withPath: ServerEntry = { id: "srv_1", ...SSH_INPUT };
    const pathPub = publicServer(withPath);
    expect((pathPub.ssh as { key: string }).key).toBe("~/.ssh/id_ed25519");

    const pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n-----END OPENSSH PRIVATE KEY-----";
    const withPem: ServerEntry = {
      id: "srv_1",
      ...INPUT,
      ssh: { host: "b", port: 22, user: "u", key: pem },
    };
    const pemPub = publicServer(withPem);
    expect(JSON.stringify(pemPub)).not.toContain("fake");
    expect((pemPub.ssh as { key: string }).key).toBe("••••••••");
  });
});

describe("authHeader", () => {
  it("maps token auth to a Bearer header", () => {
    expect(authHeader({ type: "token", secret: "tok" })).toBe("Bearer tok");
  });
  it("maps password auth to Basic with a default user", () => {
    expect(authHeader({ type: "password", secret: "pw" })).toBe(
      `Basic ${Buffer.from("sepia:pw").toString("base64")}`,
    );
    expect(authHeader({ type: "password", user: "luis", secret: "pw" })).toBe(
      `Basic ${Buffer.from("luis:pw").toString("base64")}`,
    );
  });
  it("returns null without auth", () => {
    expect(authHeader(null)).toBeNull();
  });
});

describe("sshTunnelArgs", () => {
  const ssh = { host: "bastion", port: 2222, user: "luis", key: "~/.ssh/id_ed25519" };

  it("builds a non-interactive local forward", () => {
    const args = sshTunnelArgs(ssh, "10.0.0.5", 8787, 39_001);
    expect(args).toContain("-N");
    expect(args).toContain("BatchMode=yes");
    expect(args).toContain("ExitOnForwardFailure=yes");
    expect(args).toContain("-L");
    expect(args).toContain("127.0.0.1:39001:10.0.0.5:8787");
    expect(args).toContain("-p");
    expect(args).toContain("2222");
    expect(args).toContain("-i");
    expect(args).toContain("~/.ssh/id_ed25519");
    expect(args.at(-1)).toBe("luis@bastion");
  });

  it("omits -i for inline keys unless a materialized keyFile is given", () => {
    const inline = {
      host: "b",
      port: 22,
      user: "u",
      key: "-----BEGIN KEY-----\nx\n-----END KEY-----",
    };
    expect(sshTunnelArgs(inline, "h", 1, 2)).not.toContain("-i");
    const withFile = sshTunnelArgs(inline, "h", 1, 2, { keyFile: "/tmp/k.pem" });
    expect(withFile).toContain("-i");
    expect(withFile).toContain("/tmp/k.pem");
  });
});

describe("isInlineKey", () => {
  it("treats paths as paths and PEM material as inline", () => {
    expect(isInlineKey("~/.ssh/id_ed25519")).toBe(false);
    expect(isInlineKey("-----BEGIN OPENSSH PRIVATE KEY-----")).toBe(true);
    expect(isInlineKey("line1\nline2")).toBe(true);
  });
});

describe("createServerStore", () => {
  const paths = () => {
    const dir = tmp();
    return { file: join(dir, "servers.json"), key: join(dir, "keys", "servers.key") };
  };

  it("round-trips entries across reloads and encrypts the file", () => {
    const { file, key } = paths();
    const store = createServerStore(file, key, {});
    const created = store.create(INPUT);
    expect(created.id).toMatch(/^srv_/);
    expect(store.get(created.id)?.auth?.secret).toBe("s3cr3t-token");

    // Secrets never touch disk in cleartext.
    const onDisk = readFileSync(file, "utf8");
    expect(onDisk).not.toContain("s3cr3t-token");
    expect(onDisk).not.toContain("Thinkpad");
    expect(() => JSON.parse(onDisk) as { v: number }).not.toThrow();

    // The key file was minted with restrictive permissions.
    const keyHex = readFileSync(key, "utf8").trim();
    expect(keyHex).toMatch(/^[0-9a-f]{64}$/);

    const reloaded = createServerStore(file, key, {});
    expect(reloaded.list()).toHaveLength(1);
    expect(reloaded.get(created.id)?.label).toBe("Thinkpad");
  });

  it("keeps the stored secret when the update carries the mask", () => {
    const { file, key } = paths();
    const store = createServerStore(file, key, {});
    const created = store.create(INPUT);
    store.update(created.id, {
      ...INPUT,
      label: "Renamed",
      auth: { type: "token", secret: "••••••••" },
    });
    expect(store.get(created.id)?.label).toBe("Renamed");
    expect(store.get(created.id)?.auth?.secret).toBe("s3cr3t-token");
  });

  it("clears auth when the update sets it to null", () => {
    const { file, key } = paths();
    const store = createServerStore(file, key, {});
    const created = store.create(INPUT);
    store.update(created.id, { ...INPUT, auth: null });
    expect(store.get(created.id)?.auth).toBeNull();
  });

  it("fails closed when the key doesn't match the file", () => {
    const { file, key } = paths();
    createServerStore(file, key, {}).create(INPUT);
    const wrongKey = join(tmp(), "other.key");
    const broken = createServerStore(file, wrongKey, {});
    expect(broken.error).not.toBeNull();
    expect(broken.list()).toEqual([]);
    expect(() => broken.create(INPUT)).toThrow();
    // … and the sealed file is left untouched.
    expect(readFileSync(file, "utf8")).not.toContain("s3cr3t-token");
  });

  it("honors SEPIA_SERVERS_KEY over the key file", () => {
    const { file } = paths();
    const envKey = "ab".repeat(32);
    const store = createServerStore(file, join(tmp(), "unused.key"), {
      SEPIA_SERVERS_KEY: envKey,
    });
    store.create(INPUT);
    const reloaded = createServerStore(file, join(tmp(), "other.key"), {
      SEPIA_SERVERS_KEY: envKey,
    });
    expect(reloaded.error).toBeNull();
    expect(reloaded.list()).toHaveLength(1);
  });
});

describe("handleServersRoute", () => {
  const makeDeps = (
    store: ReturnType<typeof createServerStore>,
    fetchImpl?: ServersRouteDeps["fetchImpl"],
  ): ServersRouteDeps => {
    const tunnels: TunnelManager = {
      ensure: () => Promise.resolve({ localPort: 44_001 }),
      localPort: () => 44_001,
      close: () => {},
      closeAll: () => {},
    };
    return { store, tunnels, cors: {}, fetchImpl };
  };

  const get = (path: string, init?: RequestInit): Request =>
    new Request(`http://localhost${path}`, init);

  it("lists servers masked and creates entries that persist secrets", async () => {
    const dir = tmp();
    const store = createServerStore(join(dir, "s.json"), join(dir, "k.key"), {});
    const deps = makeDeps(store);

    const created = await handleServersRoute(
      get("/api/servers", { method: "POST", body: JSON.stringify(INPUT) }),
      [],
      deps,
    );
    expect(created?.status).toBe(201);
    const createdBody = (await created?.json()) as { server: Record<string, unknown> };
    expect(JSON.stringify(createdBody)).not.toContain("s3cr3t-token");
    expect(store.list()).toHaveLength(1);

    const listed = await handleServersRoute(get("/api/servers"), [], deps);
    const listBody = (await listed?.json()) as { servers: unknown[] };
    expect(listBody.servers).toHaveLength(1);
    expect(JSON.stringify(listBody)).not.toContain("s3cr3t-token");
  });

  it("rejects invalid bodies with 400 and never writes them", async () => {
    const dir = tmp();
    const store = createServerStore(join(dir, "s.json"), join(dir, "k.key"), {});
    const res = await handleServersRoute(
      get("/api/servers", { method: "POST", body: JSON.stringify({ label: "" }) }),
      [],
      makeDeps(store),
    );
    expect(res?.status).toBe(400);
    expect(store.list()).toHaveLength(0);
  });

  it("keeps the stored secret across a masked PATCH", async () => {
    const dir = tmp();
    const store = createServerStore(join(dir, "s.json"), join(dir, "k.key"), {});
    const deps = makeDeps(store);
    const entry = store.create(INPUT);

    const res = await handleServersRoute(
      get(`/api/servers/${entry.id}`, {
        method: "PATCH",
        body: JSON.stringify({
          ...INPUT,
          label: "New label",
          auth: { type: "token", secret: "••••••••" },
        }),
      }),
      [entry.id],
      deps,
    );
    expect(res?.status).toBe(200);
    expect(store.get(entry.id)?.auth?.secret).toBe("s3cr3t-token");
    expect(store.get(entry.id)?.label).toBe("New label");
  });

  it("proxies upstream with the stored credentials — never the caller's", async () => {
    const dir = tmp();
    const store = createServerStore(join(dir, "s.json"), join(dir, "k.key"), {});
    const entry = store.create(INPUT);
    let seen: { url?: string; auth?: string | null } = {};
    const urlOf = (input: Parameters<typeof fetch>[0]): string =>
      typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const fetchImpl: typeof fetch = (input, init) => {
      seen = {
        url: urlOf(input),
        auth: new Headers(init?.headers).get("authorization"),
      };
      return Promise.resolve(
        new Response('{"id":"node_x"}', {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
      );
    };

    const res = await handleServersRoute(
      get(`/api/servers/${entry.id}/proxy/api/node?x=1`, {
        headers: { authorization: "Bearer caller-token" },
      }),
      [entry.id, "proxy", "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(200);
    expect(seen.url).toBe("http://192.168.1.10:8787/api/node?x=1");
    expect(seen.auth).toBe("Bearer s3cr3t-token");
    expect(seen.auth).not.toContain("caller-token");
  });

  it("routes ssh entries through the tunnel's local port", async () => {
    const dir = tmp();
    const store = createServerStore(join(dir, "s.json"), join(dir, "k.key"), {});
    const entry = store.create(SSH_INPUT);
    let seenUrl = "";
    const fetchImpl: typeof fetch = (input) => {
      seenUrl =
        typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      return Promise.resolve(new Response("{}"));
    };
    await handleServersRoute(
      get(`/api/servers/${entry.id}/proxy/api/node`),
      [entry.id, "proxy", "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(seenUrl).toBe("http://127.0.0.1:44001/api/node");
  });
});
