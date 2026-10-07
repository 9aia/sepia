import { Effect, Layer, Option } from "effect";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import * as ClaudeCode from "./ClaudeCode.js";
import { MessageNode, Session, StorageError } from "sepia-core";
import type { SessionRepositoryService } from "sepia-core";

export interface ClaudeCodeRepositoryOptions {
  /** Claude Code projects root, usually `~/.claude/projects`. */
  readonly projectsDir: string;
}

export interface TranscriptFile {
  readonly filePath: string;
  readonly id: string;
  readonly fallbackCwd: string;
  readonly parentSessionId: string | undefined;
}

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const storageError = (prefix: string) => (cause: unknown) =>
  new StorageError({
    message: `${prefix}: ${cause instanceof Error ? cause.message : String(cause)}`,
  });

/** Ids become path segments — no separators, NUL, or dot-dirs. */
const isSafeFileName = (name: string): boolean =>
  name !== "" && name !== "." && name !== ".." && !/[/\\\0]/.test(name);

const JSONL = ".jsonl";

/** Every transcript file under the projects root, with its provenance. */
const scanTranscriptFiles = (
  projectsDir: string,
): Effect.Effect<ReadonlyArray<TranscriptFile>, unknown, Fs.FileSystem | Path.Path> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const path = yield* Path.Path;
    const exists = yield* fs.exists(projectsDir).pipe(Effect.orElseSucceed(() => false));
    if (!exists) return [] as ReadonlyArray<TranscriptFile>;
    const dirs = yield* fs.readDirectory(projectsDir);
    const files: Array<TranscriptFile> = [];
    for (const dirName of dirs) {
      const dir = path.join(projectsDir, dirName);
      const info = yield* fs.stat(dir).pipe(Effect.option);
      if (Option.isNone(info) || info.value.type !== "Directory") continue;
      const fallbackCwd = ClaudeCode.decodeProjectDir(dirName);
      const entries = yield* fs
        .readDirectory(dir)
        .pipe(Effect.orElseSucceed(() => [] as ReadonlyArray<string>));
      for (const entry of entries) {
        if (entry.endsWith(JSONL)) {
          files.push({
            filePath: path.join(dir, entry),
            id: entry.slice(0, -JSONL.length),
            fallbackCwd,
            parentSessionId: undefined,
          });
          continue;
        }
        // A `<uuid>/` directory may hold `subagents/agent-*.jsonl`.
        const subDir = path.join(dir, entry, "subagents");
        const hasSubDir = yield* fs.exists(subDir).pipe(Effect.orElseSucceed(() => false));
        if (!hasSubDir) continue;
        const subs = yield* fs
          .readDirectory(subDir)
          .pipe(Effect.orElseSucceed(() => [] as ReadonlyArray<string>));
        for (const sub of subs) {
          if (!sub.endsWith(JSONL)) continue;
          files.push({
            filePath: path.join(subDir, sub),
            id: sub.slice(0, -JSONL.length),
            fallbackCwd,
            parentSessionId: entry,
          });
        }
      }
    }
    return files;
  });

/**
 * SessionRepository over Claude Code's on-disk transcripts. The store has
 * no manifest, so `list()` reads each `.jsonl` and summarizes it without
 * building nodes; `getById` parses the full transcript.
 *
 * Layout handled: `<projects>/<slug>/<uuid>.jsonl` main sessions plus
 * sub-agent transcripts in `<uuid>/subagents/agent-*.jsonl` (current) and
 * `agent-*.jsonl` at the project root (legacy).
 *
 * `save` writes the canonical layout: a top-level session lands at
 * `<projects>/<cwd-slug>/<id>.jsonl`; a session with `parentSessionId`
 * lands at `<slug>/<parent>/subagents/<id>.jsonl` (reusing the project
 * dir that already holds the parent when one exists). `delete` removes
 * the transcript plus its `<id>/` subagents dir.
 */
export const makeClaudeCodeSessionRepository = (
  options: ClaudeCodeRepositoryOptions,
): SessionRepositoryService => {
  const transcriptFiles = () => scanTranscriptFiles(options.projectsDir);

  return {
    list: () =>
      Effect.gen(function* () {
        const fs = yield* Fs.FileSystem;
        const sessions: Array<Session> = [];
        for (const file of yield* transcriptFiles()) {
          const raw = yield* fs.readFileString(file.filePath).pipe(Effect.orElseSucceed(() => ""));
          if (raw === "") continue;
          sessions.push(
            ClaudeCode.summarizeJsonl(raw, {
              id: file.id,
              fallbackCwd: file.fallbackCwd,
              parentSessionId: file.parentSessionId,
            }),
          );
        }
        return sessions.sort((a, b) => b.lastActivityAt - a.lastActivityAt);
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to list claude sessions")),
      ),

    getById: (id) =>
      Effect.gen(function* () {
        const file = (yield* transcriptFiles()).find((candidate) => candidate.id === id);
        if (file === undefined) return Option.none<Session>();
        // `fromFile` re-derives id, decoded cwd and subagent parentage from
        // the path — the same values the scan computed.
        const session = yield* ClaudeCode.fromFile(file.filePath);
        return Option.some(session);
      }).pipe(
        Effect.provide(fsLayer),
        Effect.catchAll((error: unknown) => {
          const message = error instanceof Error ? error.message : String(error);
          return /not found/i.test(message)
            ? Effect.succeed(Option.none<Session>())
            : Effect.fail(
                new StorageError({ message: `Failed to read claude session: ${message}` }),
              );
        }),
      ),

    hasSession: (id) =>
      transcriptFiles().pipe(
        Effect.map((files) => files.some((file) => file.id === id)),
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to check claude session")),
      ),

    save: (session) =>
      Effect.gen(function* () {
        const fs = yield* Fs.FileSystem;
        const path = yield* Path.Path;
        const parentId = Option.getOrUndefined(session.parentSessionId);
        for (const unsafe of [session.id, parentId]) {
          if (unsafe !== undefined && !isSafeFileName(unsafe)) {
            return yield* Effect.fail(
              new StorageError({
                message: `Claude session id is not a safe file name: ${JSON.stringify(unsafe)}`,
              }),
            );
          }
        }

        let filePath: string;
        if (parentId === undefined) {
          // Canonical layout — `<projects>/<cwd-slug>/<id>.jsonl`.
          const dir = path.join(
            options.projectsDir,
            ClaudeCode.encodeProjectDir(session.workingDirectory),
          );
          yield* fs.makeDirectory(dir, { recursive: true });
          filePath = path.join(dir, `${session.id}${JSONL}`);
        } else {
          // A subagent transcript lives at `<slug>/<parent>/subagents/` —
          // reuse the project that already holds the parent, else this
          // session's own project.
          let slug = ClaudeCode.encodeProjectDir(session.workingDirectory);
          for (const candidate of yield* fs
            .readDirectory(options.projectsDir)
            .pipe(Effect.orElseSucceed(() => [] as ReadonlyArray<string>))) {
            const held = yield* fs
              .exists(path.join(options.projectsDir, candidate, `${parentId}${JSONL}`))
              .pipe(Effect.orElseSucceed(() => false));
            if (held) {
              slug = candidate;
              break;
            }
          }
          const dir = path.join(options.projectsDir, slug, parentId, "subagents");
          yield* fs.makeDirectory(dir, { recursive: true });
          filePath = path.join(dir, `${session.id}${JSONL}`);
        }
        yield* fs.writeFileString(filePath, ClaudeCode.toJsonl(session));
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to save claude session")),
      ),

    delete: (id) =>
      Effect.gen(function* () {
        const fs = yield* Fs.FileSystem;
        const path = yield* Path.Path;
        if (!isSafeFileName(id)) {
          return yield* Effect.fail(
            new StorageError({
              message: `Claude session id is not a safe file name: ${JSON.stringify(id)}`,
            }),
          );
        }
        const file = (yield* transcriptFiles()).find((candidate) => candidate.id === id);
        if (file === undefined) return;
        yield* fs.remove(file.filePath);
        // A main transcript's `<id>/` dir holds its subagents — remove it
        // with the session; a subagent file has no dir of its own.
        const sideDir = path.join(path.dirname(file.filePath), id);
        const held = yield* fs.exists(sideDir).pipe(Effect.orElseSucceed(() => false));
        if (held) yield* fs.remove(sideDir, { recursive: true });
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to delete claude session")),
      ),
  };
};

const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);

/** The transcript entry's `uuid` a reader-tagged node carries. */
const nodeUuid = (node: MessageNode): string | undefined => {
  const meta = node.metadata;
  if (!isObject(meta)) return undefined;
  const uuid = meta.uuid;
  return typeof uuid === "string" && uuid !== "" ? uuid : undefined;
};

/**
 * In-place conversation rewind for a Claude Code transcript: rewrite the
 * `.jsonl` so it ends with the entry that produced the last kept node.
 *
 * The file is append-only by convention — entries can't be deleted
 * in place, so the truncation is a prefix rewrite: every node records its
 * source entry's `uuid` in `metadata.uuid`, the uuid→line map locates the
 * cut, and lines past it are dropped. Kept lines are preserved byte for
 * byte (no re-serialization), so the surviving transcript is identical to
 * what the agent wrote — including plumbing entries (`summary`,
 * `file-history-snapshot`, `queue-operation`) that emit no node but sit
 * before the cut.
 *
 * One JSONL entry can emit several nodes (a `user` entry's tool_result
 * blocks become `tool` nodes); a node removed from an entry the cut
 * keeps is a boundary survivor — the entry can't be split. A removed
 * node on an *earlier* line than the cut means the file's order can't
 * honor the requested boundary and the truncation refuses instead of
 * silently over-keeping.
 */
export const truncateClaudeTranscript = (
  options: ClaudeCodeRepositoryOptions,
  sessionId: string,
  kept: ReadonlyArray<MessageNode>,
  removed: ReadonlyArray<MessageNode>,
): Effect.Effect<{ readonly removedEntries: number }, StorageError> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const files = yield* scanTranscriptFiles(options.projectsDir);
    const file = files.find((candidate) => candidate.id === sessionId);
    if (file === undefined) {
      return yield* Effect.fail(
        new StorageError({ message: `Claude transcript not found: ${sessionId}` }),
      );
    }
    const raw = yield* fs.readFileString(file.filePath);
    const lines = raw.split("\n");
    if (lines[lines.length - 1] === "") lines.pop();

    // uuid → line index; entries without one (or that don't parse) are
    // plumbing — they can't anchor a node either way.
    const lineByUuid = new Map<string, number>();
    lines.forEach((line, index) => {
      try {
        const entry: unknown = JSON.parse(line);
        if (isObject(entry) && typeof entry.uuid === "string") {
          lineByUuid.set(entry.uuid, index);
        }
      } catch {
        // Non-JSON lines can't have produced a node; they ride the cut.
      }
    });

    const lineOf = (node: MessageNode): Effect.Effect<number, StorageError> => {
      const uuid = nodeUuid(node);
      const line = uuid === undefined ? undefined : lineByUuid.get(uuid);
      return line === undefined
        ? Effect.fail(
            new StorageError({
              message: `Node ${node.nodeId} has no locatable transcript entry — cannot truncate`,
            }),
          )
        : Effect.succeed(line);
    };
    const keptLines: Array<number> = [];
    for (const node of kept) keptLines.push(yield* lineOf(node));
    const removedLines: Array<number> = [];
    for (const node of removed) removedLines.push(yield* lineOf(node));

    const keptSet = new Set(keptLines);
    const cutLine = keptLines.length === 0 ? 0 : Math.max(...keptLines) + 1;
    for (const line of removedLines) {
      if (line < cutLine && !keptSet.has(line)) {
        return yield* Effect.fail(
          new StorageError({
            message:
              `The transcript's entry order cannot honor this cut — ` +
              `a removed node sits at line ${line + 1}, inside the kept prefix`,
          }),
        );
      }
    }
    if (cutLine >= lines.length) return { removedEntries: 0 };

    const trailingNewline = raw.endsWith("\n") ? "\n" : "";
    const tmp = `${file.filePath}.tmp-${process.pid}`;
    yield* fs.writeFileString(tmp, lines.slice(0, cutLine).join("\n") + trailingNewline);
    yield* fs.rename(tmp, file.filePath);
    return { removedEntries: lines.length - cutLine };
  }).pipe(
    Effect.provide(fsLayer),
    Effect.mapError((error) =>
      error instanceof StorageError
        ? error
        : storageError("Failed to truncate claude session")(error),
    ),
  );
