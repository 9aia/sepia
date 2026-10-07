/**
 * systemd.ts — the ServiceBackend is exercised against fake `systemctl`/
 * `journalctl` shell stubs on a scratch PATH, plus a fake $HOME so the user
 * unit lands under tmp. `Bun.spawnSync` is shimmed over node:child_process
 * (vitest runs under node — same pattern as apps/server/tests/ui.test.ts).
 */
import { spawnSync as nodeSpawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, test } from "vite-plus/test";
import { defaultEnvFile, envTemplate, type ServiceSpec } from "../src/service/backend.js";
import { systemdBackend } from "../src/service/systemd.js";

const realBun = (globalThis as { Bun?: unknown }).Bun;

/** Minimal `Bun.spawnSync` stand-in — same argv/stdio/env contract. */
const bunShim = {
  spawnSync: (
    cmd: ReadonlyArray<string>,
    opts?: {
      stdin?: "ignore" | "inherit" | "pipe";
      stdout?: "ignore" | "inherit" | "pipe";
      stderr?: "ignore" | "inherit" | "pipe";
      env?: Record<string, string | undefined>;
    },
  ) => {
    const res = nodeSpawnSync(cmd[0], cmd.slice(1), {
      stdio: [opts?.stdin ?? "ignore", opts?.stdout ?? "pipe", opts?.stderr ?? "pipe"],
      env: opts?.env,
    });
    return {
      exitCode: res.error ? 127 : (res.status ?? 1),
      stdout: res.stdout ?? Buffer.alloc(0),
      stderr: res.stderr ?? (res.error ? Buffer.from(res.error.message) : Buffer.alloc(0)),
    };
  },
};

// Fake systemctl: logs every call, models enable/active state as marker files
// under $SEPIA_FAKE/state so the backend's parses see realistic stdout/exits.
const SYSTEMCTL_STUB = `#!/bin/sh
echo "systemctl $*" >> "$SEPIA_FAKE/calls"
state="$SEPIA_FAKE/state"
case " $* " in
*" is-enabled "*)
  if [ -f "$state/enabled" ]; then echo enabled; exit 0; fi
  echo "Failed to get unit file state for sepia.service: No such file or directory" >&2
  exit 1
  ;;
*" is-active "*)
  if [ -f "$state/active" ]; then echo active; exit 0; fi
  echo inactive
  exit 3
  ;;
*" show "*)
  if [ -f "$state/active" ]; then echo "MainPID=4242"; else echo "MainPID=0"; fi
  exit 0
  ;;
*" daemon-reload "*)
  exit 0
  ;;
*" enable "*)
  if [ "$SEPIA_FAKE_FAIL_ENABLE" = "1" ]; then
    echo "Failed to enable unit: simulated failure" >&2
    exit 1
  fi
  touch "$state/enabled" "$state/active"
  exit 0
  ;;
*" disable "*)
  rm -f "$state/enabled" "$state/active"
  exit 0
  ;;
*" restart "*)
  if [ -f "$state/enabled" ]; then touch "$state/active"; exit 0; fi
  echo "Failed to restart sepia.service: Unit sepia.service not found." >&2
  exit 1
  ;;
esac
exit 0
`;

const JOURNALCTL_STUB = `#!/bin/sh
echo "journalctl $*" >> "$SEPIA_FAKE/calls"
cat "$SEPIA_FAKE/journal" 2>/dev/null
exit "\${SEPIA_FAKE_JOURNAL_EXIT:-0}"
`;

const root = mkdtempSync(join(tmpdir(), "sepia-systemd-"));
const home = join(root, "home");
const fake = join(root, "fake");
const bin = join(fake, "bin");
const state = join(fake, "state");
const origPath = process.env.PATH ?? "";
const origHome = process.env.HOME;

const userSpec = (): ServiceSpec => ({
  exec: ["/usr/local/bin/sepia", "serve"],
  envFile: defaultEnvFile(home),
  system: false,
});

const calls = (): Array<string> =>
  (existsSync(join(fake, "calls")) ? readFileSync(join(fake, "calls"), "utf-8") : "")
    .split("\n")
    .filter((line) => line !== "");

const unitFile = (spec: ServiceSpec) => systemdBackend.unitPath(spec);

beforeAll(() => {
  (globalThis as { Bun?: unknown }).Bun = bunShim;
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(bin, "systemctl"), SYSTEMCTL_STUB);
  writeFileSync(join(bin, "journalctl"), JOURNALCTL_STUB);
  chmodSync(join(bin, "systemctl"), 0o755);
  chmodSync(join(bin, "journalctl"), 0o755);
});

// Full isolation per test: fake PATH/HOME, clean state markers, clean $HOME.
beforeEach(() => {
  process.env.PATH = `${bin}${delimiter}${origPath}`;
  process.env.HOME = home;
  process.env.SEPIA_FAKE = fake;
  delete process.env.SEPIA_FAKE_FAIL_ENABLE;
  delete process.env.SEPIA_FAKE_JOURNAL_EXIT;
  rmSync(home, { recursive: true, force: true });
  rmSync(state, { recursive: true, force: true });
  rmSync(join(fake, "calls"), { force: true });
  mkdirSync(home, { recursive: true });
  mkdirSync(state, { recursive: true });
  writeFileSync(join(fake, "journal"), "line 1\nline 2\nline 3\n");
});

afterAll(() => {
  process.env.PATH = origPath;
  if (origHome === undefined) delete process.env.HOME;
  else process.env.HOME = origHome;
  delete process.env.SEPIA_FAKE;
  delete process.env.SEPIA_FAKE_FAIL_ENABLE;
  delete process.env.SEPIA_FAKE_JOURNAL_EXIT;
  (globalThis as { Bun?: unknown }).Bun = realBun;
  rmSync(root, { recursive: true, force: true });
});

describe("systemd render", () => {
  test("user unit renders with %h env file and default.target", () => {
    expect(systemdBackend.render(userSpec())).toMatchInlineSnapshot(`
      "[Unit]
      Description=Sepia node
      After=network-online.target

      [Service]
      ExecStart=/usr/local/bin/sepia serve
      EnvironmentFile=-%h/.config/sepia/env
      Restart=on-failure
      RestartSec=2

      [Install]
      WantedBy=default.target
      "
    `);
  });

  test("system unit renders with the literal env file and multi-user.target", () => {
    const spec: ServiceSpec = {
      exec: ["/usr/local/bin/sepia", "serve"],
      envFile: "/etc/sepia/env",
      system: true,
    };
    expect(systemdBackend.render(spec)).toMatchInlineSnapshot(`
      "[Unit]
      Description=Sepia node
      After=network-online.target

      [Service]
      ExecStart=/usr/local/bin/sepia serve
      EnvironmentFile=-/etc/sepia/env
      Restart=on-failure
      RestartSec=2

      [Install]
      WantedBy=multi-user.target
      "
    `);
  });

  test("exec args containing spaces are double-quoted", () => {
    const spec: ServiceSpec = {
      exec: ["/opt/sepia dist/sepia", "serve", "--port 9000"],
      envFile: defaultEnvFile(home),
      system: false,
    };
    const rendered = systemdBackend.render(spec);
    expect(rendered).toContain('ExecStart="/opt/sepia dist/sepia" serve "--port 9000"');
  });

  test("unitPath resolves per spec.system", () => {
    expect(unitFile(userSpec())).toBe(
      join(homedir(), ".config", "systemd", "user", "sepia.service"),
    );
    expect(systemdBackend.unitPath({ exec: [], envFile: "/etc/sepia/env", system: true })).toBe(
      "/etc/systemd/system/sepia.service",
    );
  });
});

describe("systemd verbs against stub binaries", () => {
  test("install writes the unit + env template, then daemon-reloads and enables", async () => {
    const spec = userSpec();
    await systemdBackend.install(spec);

    expect(readFileSync(unitFile(spec), "utf-8")).toBe(systemdBackend.render(spec));
    expect(readFileSync(spec.envFile, "utf-8")).toBe(envTemplate());
    expect(calls()).toEqual([
      "systemctl --user daemon-reload",
      "systemctl --user reset-failed sepia",
      "systemctl --user enable --now sepia",
    ]);
  });

  test("install never overwrites an existing env file", async () => {
    const spec = userSpec();
    mkdirSync(join(spec.envFile, ".."), { recursive: true });
    writeFileSync(spec.envFile, "SEPIA_TOKEN=kept\n");
    await systemdBackend.install(spec);

    expect(readFileSync(spec.envFile, "utf-8")).toBe("SEPIA_TOKEN=kept\n");
    // reinstall stays idempotent too
    await systemdBackend.install(spec);
    expect(readFileSync(spec.envFile, "utf-8")).toBe("SEPIA_TOKEN=kept\n");
  });

  test("install throws with the child's stderr on nonzero exit", async () => {
    process.env.SEPIA_FAKE_FAIL_ENABLE = "1";
    await expect(systemdBackend.install(userSpec())).rejects.toThrow(
      "Failed to enable unit: simulated failure",
    );
  });

  test("install fails fast when systemctl is not on PATH", async () => {
    process.env.PATH = mkdtempSync(join(root, "empty-path-"));
    await expect(systemdBackend.install(userSpec())).rejects.toThrow("systemd not available");
  });

  test("uninstall disables, removes the unit and reloads; purge drops the env file", async () => {
    const spec = userSpec();
    await systemdBackend.install(spec);
    rmSync(join(fake, "calls"), { force: true });

    await systemdBackend.uninstall(spec, false);
    expect(calls()).toEqual([
      "systemctl --user disable --now sepia",
      "systemctl --user daemon-reload",
    ]);
    expect(existsSync(unitFile(spec))).toBe(false);
    expect(existsSync(spec.envFile)).toBe(true);

    await systemdBackend.install(spec);
    await systemdBackend.uninstall(spec, true);
    expect(existsSync(unitFile(spec))).toBe(false);
    expect(existsSync(spec.envFile)).toBe(false);
  });

  test("uninstall tolerates a missing unit", async () => {
    await expect(systemdBackend.uninstall(userSpec(), false)).resolves.toBeUndefined();
  });

  test("status reports not-installed before install and enabled/active/pid after", async () => {
    const spec = userSpec();
    const before = await systemdBackend.status(spec);
    expect(before).toEqual({ installed: false, enabled: false, active: false, detail: "" });

    await systemdBackend.install(spec);
    rmSync(join(fake, "calls"), { force: true });
    const after = await systemdBackend.status(spec);
    expect(after.installed).toBe(true);
    expect(after.enabled).toBe(true);
    expect(after.active).toBe(true);
    expect(after.pid).toBe(4242);
    expect(after.detail).toBe("line 1\nline 2\nline 3");
    expect(calls()).toEqual([
      "systemctl --user is-enabled sepia",
      "systemctl --user is-active sepia",
      "systemctl --user show sepia -p MainPID",
      "journalctl --user -u sepia -n 10 --no-pager",
    ]);
  });

  test("status degrades to not-installed when systemctl is missing", async () => {
    process.env.PATH = mkdtempSync(join(root, "empty-path-"));
    await expect(systemdBackend.status(userSpec())).resolves.toEqual({
      installed: false,
      enabled: false,
      active: false,
      detail: "",
    });
  });

  test("restart calls systemctl --user restart and propagates failure", async () => {
    const spec = userSpec();
    await systemdBackend.install(spec);
    rmSync(join(fake, "calls"), { force: true });

    await systemdBackend.restart(spec);
    expect(calls()).toEqual(["systemctl --user restart sepia"]);

    await systemdBackend.uninstall(spec, false);
    await expect(systemdBackend.restart(spec)).rejects.toThrow(
      "Failed to restart sepia.service: Unit sepia.service not found.",
    );
  });

  test("logs streams journalctl with inherited stdio and returns its exit code", async () => {
    const spec = userSpec();
    await expect(systemdBackend.logs(spec, false)).resolves.toBe(0);
    await expect(systemdBackend.logs(spec, true)).resolves.toBe(0);
    expect(calls()).toEqual(["journalctl --user -u sepia", "journalctl --user -u sepia -f"]);

    process.env.SEPIA_FAKE_JOURNAL_EXIT = "5";
    await expect(systemdBackend.logs(spec, false)).resolves.toBe(5);
  });
});
