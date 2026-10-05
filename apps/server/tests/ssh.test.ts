/**
 * ssh.ts — the real ssh tunnel manager. `ssh` shells out to the system
 * binary; a nonexistent host fails fast under BatchMode, so the spawn +
 * waitForPort + failure-surface path runs for real.
 */
import { existsSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, expect, test } from "vite-plus/test";
import type { ServerEntry } from "../src/servers";
import { createTunnelManager, sshTunnelArgs } from "../src/ssh";

const dir = mkdtempSync(join(tmpdir(), "sepia-ssh-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));

const entry = (over: Partial<ServerEntry> = {}): ServerEntry => ({
  id: "srv-1",
  label: "peer",
  host: "127.0.0.1",
  port: 22,
  scheme: "http",
  auth: null,
  ssh: { host: "127.0.0.1", port: 1, user: "nobody", key: undefined },
  ...over,
});

test("sshTunnelArgs builds the argv for a key-file and inline-key entry", () => {
  const args = sshTunnelArgs(
    { host: "h", port: 2222, user: "me", key: "/keys/k" },
    "target",
    9090,
    8080,
  );
  expect(args).toContain("-N");
  expect(args).toContain("127.0.0.1:8080:target:9090");
  expect(args).toEqual(expect.arrayContaining(["-i", "/keys/k", "me@h"]));
  // an inline PEM never becomes -i — it is materialized instead
  const inline = sshTunnelArgs(
    { host: "h", port: 22, user: "me", key: "-----BEGIN KEY-----\nx" },
    "t",
    1,
    2,
  );
  expect(inline).not.toContain("-i");
  // an explicit override wins over the stored path
  expect(
    sshTunnelArgs({ host: "h", port: 22, user: "me", key: "/k" }, "t", 1, 2, {
      keyFile: "/other",
    }),
  ).toContain("/other");
});

test("ensure fails cleanly for ssh-less and unreachable entries", async () => {
  const manager = createTunnelManager({ keyDir: join(dir, "keys") });
  await expect(manager.ensure(entry({ ssh: null }))).rejects.toThrow("no SSH config");
  // an inline key is materialized with owner-only perms before spawn
  const keyed = entry({
    ssh: { host: "127.0.0.1", port: 1, user: "nobody", key: "-----BEGIN-----\nk\n" },
  });
  await expect(manager.ensure(keyed)).rejects.toThrow(/SSH tunnel failed|exited|did not come up/);
  const pem = join(dir, "keys", "srv-1.pem");
  expect(existsSync(pem)).toBe(true);
  expect(statSync(pem).mode & 0o777).toBe(0o600);
  // the failure is not cached — a second attempt re-attempts
  await expect(manager.ensure(keyed)).rejects.toThrow();
  // nothing live: localPort/close/closeAll are safe no-ops
  expect(manager.localPort("srv-1")).toBeUndefined();
  manager.close("srv-1");
  manager.closeAll();
});

test("an ssh that hangs mid-connect times out the port probe and is killed", async () => {
  // 10.255.255.1 is unroutable — ssh stays in TCP connect (no fast exit), so
  // the local-port probe retries until the 8s deadline, then SIGKILLs.
  const manager = createTunnelManager({ keyDir: join(dir, "keys2") });
  const hanging = entry({
    ssh: { host: "10.255.255.1", port: 22, user: "nobody", key: undefined },
  });
  await expect(manager.ensure(hanging)).rejects.toThrow(
    /did not come up in time|SSH tunnel failed/,
  );
}, 15_000);
