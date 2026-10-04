import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import pkg from "../package.json";

export const SEPIA_VERSION: string = typeof pkg.version === "string" ? pkg.version : "0.0.0";

/** Sepia protocol revision — bump on breaking changes (docs/protocol.md). */
export const PROTOCOL_VERSION = 1;

export interface NodeIdentity {
  readonly id: string;
  readonly name: string;
  readonly version: string;
}

/**
 * Stable per-machine identity for federation (docs/protocol.md). The id is
 * generated on first boot and persisted next to the other sepia stores
 * (`$SEPIA_HOME/node.json`); afterwards it is read back, never regenerated.
 * Written atomically like the meta store so a crash mid-write can't leave a
 * half file.
 */
export const loadNodeIdentity = (path: string, name: string): NodeIdentity => {
  if (existsSync(path)) {
    try {
      const parsed = JSON.parse(readFileSync(path, "utf8")) as unknown;
      if (typeof parsed === "object" && parsed !== null) {
        const record = parsed as Record<string, unknown>;
        if (typeof record.id === "string" && record.id !== "") {
          return {
            id: record.id,
            name: typeof record.name === "string" && record.name !== "" ? record.name : name,
            version: SEPIA_VERSION,
          };
        }
      }
    } catch {
      // A corrupt file falls through to a fresh identity — the alternative
      // (crashing on boot) is worse than minting a new id once.
    }
  }
  const identity: NodeIdentity = {
    id: `node_${randomUUID().replaceAll("-", "").slice(0, 16)}`,
    name,
    version: SEPIA_VERSION,
  };
  mkdirSync(dirname(path), { recursive: true });
  const tmp = `${path}.tmp-${process.pid}`;
  writeFileSync(tmp, JSON.stringify({ id: identity.id, name: identity.name }));
  renameSync(tmp, path);
  return identity;
};
