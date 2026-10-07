import {
  chmodSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, test, vi } from "vite-plus/test";
import type { ServiceSpec } from "../src/service/backend.js";

/**
 * The backend resolves paths through os.homedir() — repoint it at a scratch
 * dir so nothing under the real ~/Library is touched. Everything else in
 * node:os stays real (tmpdir below included).
 */
const fake = vi.hoisted(() => ({ home: "" }));
vi.mock("node:os", async (importOriginal) => {
  const mod = await importOriginal<typeof import("node:os")>();
  return { ...mod, homedir: () => fake.home };
});

const { launchdBackend } = await import("../src/service/launchd.js");

/**
 * A fake `launchctl` on PATH — records every argv to $SEP_STUB_LOG and keeps
 * a "loaded" marker at $SEP_STUB_STATE so bootstrap/bootout/print/kickstart
 * behave like a real domain would.
 */
const STUB = `#!/bin/sh
printf '%s\\n' "$*" >> "$SEP_STUB_LOG"
cmd="$1"
shift
case "$cmd" in
  bootstrap)
    if [ -f "$SEP_STUB_STATE" ]; then
      echo "Bootstrap failed: 5" >&2
      exit 5
    fi
    touch "$SEP_STUB_STATE"
    ;;
  bootout)
    if [ ! -f "$SEP_STUB_STATE" ]; then
      echo "Could not find service" >&2
      exit 3
    fi
    rm -f "$SEP_STUB_STATE"
    ;;
  print)
    if [ ! -f "$SEP_STUB_STATE" ]; then
      echo "Could not find service $1" >&2
      exit 113
    fi
    printf '%s = {\\n\\tstate = running\\n\\tpid = 4321\\n\\tprogram = /usr/local/bin/sepia\\n}\\n' "$1"
    ;;
  kickstart)
    if [ ! -f "$SEP_STUB_STATE" ]; then
      echo "Could not find service" >&2
      exit 3
    fi
    ;;
esac
exit 0
`;

const uid = process.getuid?.() ?? 0;
const userDomain = `gui/${uid}`;
const userTarget = `${userDomain}/ai.sepia`;

let root = "";
let stubBin = "";
let stubLog = "";
let stubState = "";
let originalPath: string | undefined;

const userSpec = (envFile?: string): ServiceSpec => ({
  exec: ["/usr/local/bin/sepia", "serve"],
  envFile: envFile ?? join(fake.home, ".config", "sepia", "env"),
  system: false,
});

const systemSpec = (envFile: string): ServiceSpec => ({
  exec: ["/usr/local/bin/sepia", "serve", "--no-ui"],
  envFile,
  system: true,
});

const plistPath = () => join(fake.home, "Library", "LaunchAgents", "ai.sepia.plist");
const envPath = () => join(fake.home, ".config", "sepia", "env");
const logFilePath = () => join(fake.home, "Library", "Logs", "sepia.log");
const invocations = (): ReadonlyArray<string> =>
  existsSync(stubLog) ? readFileSync(stubLog, "utf-8").split("\n").filter(Boolean) : [];

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), "sepia-launchd-"));
  fake.home = join(root, "home");
  mkdirSync(fake.home, { recursive: true });
  stubBin = join(root, "bin");
  mkdirSync(stubBin, { recursive: true });
  writeFileSync(join(stubBin, "launchctl"), STUB);
  chmodSync(join(stubBin, "launchctl"), 0o755);
  stubLog = join(root, "launchctl.log");
  stubState = join(root, "loaded");
  process.env.SEP_STUB_LOG = stubLog;
  process.env.SEP_STUB_STATE = stubState;
  originalPath = process.env.PATH;
});

beforeEach(() => {
  rmSync(stubLog, { force: true });
  rmSync(stubState, { force: true });
  process.env.PATH = `${stubBin}${delimiter}${originalPath ?? ""}`;
});

afterAll(() => {
  process.env.PATH = originalPath;
  rmSync(root, { recursive: true, force: true });
});

describe("launchd render", () => {
  test("user plist has LaunchAgent path and no env block without an env file", () => {
    const spec = userSpec(join(fake.home, "no-such-env"));
    expect(launchdBackend.unitPath(spec)).toBe(
      join(fake.home, "Library", "LaunchAgents", "ai.sepia.plist"),
    );
    const plist = launchdBackend.render(spec);
    expect(plist).not.toContain("EnvironmentVariables");
    expect(plist).toContain(join(fake.home, "Library", "Logs", "sepia.log"));
    // the tmpdir prefix isn't stable across runs — normalize it for the snapshot
    expect(plist.replaceAll(fake.home, "~")).toMatchSnapshot();
  });

  test("user plist renders KEY=value lines, ignoring comments and blanks", () => {
    const envFile = join(fake.home, "env-with-vars");
    writeFileSync(
      envFile,
      [
        "# a comment",
        "",
        "SEPIA_TOKEN=t0ken&<x>",
        "SEPIA_HOST=127.0.0.1",
        "   ",
        "not a kv line",
        "1BAD=skipped",
        "DUP=first",
        "DUP=last",
        "# trailing comment",
      ].join("\n"),
    );
    const plist = launchdBackend.render(userSpec(envFile));
    expect(plist).toContain("<key>SEPIA_TOKEN</key>");
    expect(plist).toContain("<string>t0ken&amp;&lt;x&gt;</string>");
    expect(plist).toContain("<key>SEPIA_HOST</key>");
    expect(plist).not.toContain("not a kv line");
    expect(plist).not.toContain("1BAD");
    expect(plist).toContain("<string>last</string>");
    expect(plist).not.toContain("<string>first</string>");
    expect(plist.replaceAll(fake.home, "~")).toMatchSnapshot();
  });

  test("system plist targets LaunchDaemons and /var/log", () => {
    const spec = systemSpec(join(fake.home, "missing-env"));
    expect(launchdBackend.unitPath(spec)).toBe("/Library/LaunchDaemons/ai.sepia.plist");
    const plist = launchdBackend.render(spec);
    expect(plist).toContain("<string>/var/log/sepia.log</string>");
    expect(plist).toMatchSnapshot();
  });

  test("missing env file renders a valid plist without EnvironmentVariables", () => {
    const plist = launchdBackend.render(userSpec(join(fake.home, "gone")));
    expect(plist).toContain("<key>Label</key>");
    expect(plist).not.toContain("EnvironmentVariables");
  });
});

describe("launchd install/uninstall", () => {
  test("install writes env template + plist, then bootstraps the gui domain", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      await launchdBackend.install(userSpec());
      expect(readFileSync(envPath(), "utf-8")).toContain("SEPIA_TOKEN");
      expect(existsSync(plistPath())).toBe(true);
      expect(readFileSync(plistPath(), "utf-8")).toContain("<string>ai.sepia</string>");
      expect(invocations()).toEqual([
        `print ${userTarget}`,
        `bootstrap ${userDomain} ${plistPath()}`,
      ]);
      expect(logSpy).toHaveBeenCalledWith(
        `launchd: $ launchctl bootstrap ${userDomain} ${plistPath()}`,
      );
    } finally {
      logSpy.mockRestore();
    }
  });

  test("re-install bootouts before bootstrap and re-renders env into the plist", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      await launchdBackend.install(userSpec());
      // user edits the env file — install must not clobber it, but the new
      // values must land in the re-rendered plist.
      writeFileSync(envPath(), "SEPIA_TOKEN=from-user\n");
      await launchdBackend.install(userSpec());
      expect(readFileSync(envPath(), "utf-8")).toBe("SEPIA_TOKEN=from-user\n");
      const plist = readFileSync(plistPath(), "utf-8");
      expect(plist).toContain("<key>SEPIA_TOKEN</key>");
      expect(plist).toContain("<string>from-user</string>");

      const calls = invocations();
      const bootoutAt = calls.findIndex((c) => c.startsWith("bootout"));
      const lastBootstrap = calls.lastIndexOf(`bootstrap ${userDomain} ${plistPath()}`);
      expect(bootoutAt).toBeGreaterThan(-1);
      expect(bootoutAt).toBeLessThan(lastBootstrap);
      expect(calls[bootoutAt]).toBe(`bootout ${userDomain} ${plistPath()}`);
    } finally {
      logSpy.mockRestore();
    }
  });

  test("install fails when launchctl is not on PATH", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      process.env.PATH = join(root, "empty-bin");
      await expect(launchdBackend.install(userSpec())).rejects.toThrow("launchctl");
    } finally {
      logSpy.mockRestore();
    }
  });

  test("uninstall bootouts and removes the plist; purge drops the env file", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      await launchdBackend.install(userSpec());
      await launchdBackend.uninstall(userSpec(), false);
      expect(invocations().at(-1)).toBe(`bootout ${userDomain} ${plistPath()}`);
      expect(existsSync(plistPath())).toBe(false);
      expect(existsSync(envPath())).toBe(true);

      await launchdBackend.uninstall(userSpec(), true);
      expect(existsSync(envPath())).toBe(false);
    } finally {
      logSpy.mockRestore();
    }
  });

  test("uninstall tolerates a not-loaded job", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      // plist on disk but never bootstrapped — bootout exits nonzero, tolerated.
      mkdirSync(join(fake.home, "Library", "LaunchAgents"), { recursive: true });
      writeFileSync(plistPath(), "plist");
      await launchdBackend.uninstall(userSpec(), false);
      expect(existsSync(plistPath())).toBe(false);
      expect(invocations()).toEqual([`bootout ${userDomain} ${plistPath()}`]);
    } finally {
      logSpy.mockRestore();
    }
  });
});

describe("launchd status/restart/logs", () => {
  test("status parses pid and state from `launchctl print`", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      await launchdBackend.install(userSpec());
      mkdirSync(join(fake.home, "Library", "Logs"), { recursive: true });
      writeFileSync(
        logFilePath(),
        Array.from({ length: 15 }, (_, i) => `line ${i + 1}`).join("\n"),
      );
      const status = await launchdBackend.status(userSpec());
      expect(status.installed).toBe(true);
      expect(status.enabled).toBe(true);
      expect(status.active).toBe(true);
      expect(status.pid).toBe(4321);
      // last ~10 lines of StandardOutPath
      expect(status.detail.split("\n")).toHaveLength(10);
      expect(status.detail).toContain("line 15");
      expect(status.detail).not.toContain("line 5");
      expect(invocations().at(-1)).toBe(`print ${userTarget}`);
    } finally {
      logSpy.mockRestore();
    }
  });

  test("status reports not-installed when the job is unknown to launchd", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      const status = await launchdBackend.status(userSpec());
      expect(status.installed).toBe(false);
      expect(status.active).toBe(false);
      expect(status.pid).toBeUndefined();
    } finally {
      logSpy.mockRestore();
    }
  });

  test("status reports not-installed without launchctl on PATH", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      process.env.PATH = join(root, "empty-bin");
      const status = await launchdBackend.status(userSpec());
      expect(status.installed).toBe(false);
      expect(invocations()).toEqual([]);
    } finally {
      logSpy.mockRestore();
    }
  });

  test("restart kickstarts the service endpoint", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      await launchdBackend.install(userSpec());
      await launchdBackend.restart(userSpec());
      expect(invocations().at(-1)).toBe(`kickstart -k ${userTarget}`);
    } finally {
      logSpy.mockRestore();
    }
  });

  test("restart fails when the job is not loaded", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      await expect(launchdBackend.restart(userSpec())).rejects.toThrow("kickstart");
    } finally {
      logSpy.mockRestore();
    }
  });

  test("logs prints the last ~50 lines of the log file", async () => {
    mkdirSync(join(fake.home, "Library", "Logs"), { recursive: true });
    writeFileSync(logFilePath(), Array.from({ length: 60 }, (_, i) => `entry ${i + 1}`).join("\n"));
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      const code = await launchdBackend.logs(userSpec(), false);
      expect(code).toBe(0);
      const printed = logSpy.mock.calls.map((call) => String(call[0])).join("\n");
      expect(printed).toContain("entry 60");
      expect(printed).toContain("entry 11");
      expect(printed).not.toContain("entry 10");
    } finally {
      logSpy.mockRestore();
    }
  });
});
