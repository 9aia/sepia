import { hostname, homedir } from "node:os";
import { describe, expect, it } from "vite-plus/test";
import { parseEnv } from "../src/env";

describe("parseEnv — remaining branches", () => {
  it("defaults SEPIA_DB to the devin CLI store under homedir", () => {
    const env = parseEnv({});
    expect(env.dbPath).toBe(`${homedir()}/.local/share/devin/cli/sessions.db`);
  });

  it("an empty PORT falls back to 8787", () => {
    expect(parseEnv({ PORT: "" }).port).toBe(8787);
  });

  it("rejects a fractional PORT", () => {
    expect(() => parseEnv({ PORT: "8787.5" })).toThrow(/PORT/);
  });

  it("honors SEPIA_HOST, SEPIA_TOKEN, SEPIA_NAME", () => {
    const env = parseEnv({
      SEPIA_HOST: "0.0.0.0",
      SEPIA_TOKEN: "secret",
      SEPIA_NAME: "build-box",
    });
    expect(env.host).toBe("0.0.0.0");
    expect(env.token).toBe("secret");
    expect(env.nodeName).toBe("build-box");
  });

  it("an empty SEPIA_NAME falls back to the hostname", () => {
    expect(parseEnv({ SEPIA_NAME: "  " }).nodeName).toBe(hostname());
    expect(parseEnv({}).nodeName).toBe(hostname());
  });

  it("derives meta and node paths from SEPIA_HOME, with explicit overrides", () => {
    const env = parseEnv({ SEPIA_HOME: "/data/sepia" });
    expect(env.home).toBe("/data/sepia");
    expect(env.metaPath).toBe("/data/sepia/meta.json");
    expect(env.nodePath).toBe("/data/sepia/node.json");

    const explicit = parseEnv({
      SEPIA_HOME: "/data/sepia",
      SEPIA_META: "/elsewhere/meta.json",
      SEPIA_NODE: "/elsewhere/node.json",
    });
    expect(explicit.metaPath).toBe("/elsewhere/meta.json");
    expect(explicit.nodePath).toBe("/elsewhere/node.json");
  });

  it("trims and drops empty entries in SEPIA_ORIGINS", () => {
    const env = parseEnv({ SEPIA_ORIGINS: " https://a ,, https://b " });
    expect(env.origins).toEqual(["https://a", "https://b"]);
  });

  it("otel: enabled by default, endpoint trailing slashes stripped", () => {
    const env = parseEnv({ OTEL_EXPORTER_OTLP_ENDPOINT: "https://otel.example/" });
    expect(env.otel).toEqual({
      enabled: true,
      endpoint: "https://otel.example",
      serviceName: "sepia-server",
    });
  });

  it("otel: SEPIA_OTEL=0 disables and skips endpoint validation", () => {
    const env = parseEnv({
      SEPIA_OTEL: "0",
      OTEL_EXPORTER_OTLP_ENDPOINT: "not-a-url",
    });
    expect(env.otel.enabled).toBe(false);
    expect(env.otel.endpoint).toBe("not-a-url");
  });

  it("otel: rejects a non-http(s) endpoint while enabled", () => {
    expect(() => parseEnv({ OTEL_EXPORTER_OTLP_ENDPOINT: "grpc://x" })).toThrow(
      /OTEL_EXPORTER_OTLP_ENDPOINT/,
    );
  });

  it("otel: OTEL_SERVICE_NAME overrides the default service name", () => {
    expect(parseEnv({ OTEL_SERVICE_NAME: "sepia-prod" }).otel.serviceName).toBe("sepia-prod");
  });
});
