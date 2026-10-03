import { Console, Effect, Option } from "effect";
import * as Cline from "./Cline.js";
import { ClineStore } from "./ClineStore.js";
import { ConversionError, Session } from "./Domain.js";
import { SessionRepository } from "./Storage.js";

/** A cogs blob is usable when its `core/model` cog resolves to a real model. */
const modelCog = (cogsJson: string): unknown => {
  try {
    const cogs = JSON.parse(cogsJson);
    if (!Array.isArray(cogs)) return null;
    return (
      cogs.find(
        (c) =>
          c?.lifetime?.Unique === "core/model" &&
          typeof c?.model === "string" &&
          c.model.length > 0,
      ) ?? null
    );
  } catch {
    return null;
  }
};

/** Fill the stub `core/model` cog's model when no donor session exists. */
const withModel = (cogsJson: string, model: string): string => {
  try {
    const cogs = JSON.parse(cogsJson);
    if (Array.isArray(cogs)) {
      for (const c of cogs) {
        if (c?.lifetime?.Unique === "core/model") c.model = model;
      }
      return JSON.stringify(cogs);
    }
  } catch {}
  return cogsJson;
};

export const importCline = (
  clineDir: string,
  sessionId?: string,
  options?: { readonly dryRun?: boolean },
) =>
  Effect.gen(function* () {
    const session = yield* Cline.fromDirectory(clineDir, sessionId);
    const repo = yield* SessionRepository;

    const storedId = session.id;
    const exists = yield* repo.hasSession(storedId);
    if (exists) {
      yield* Console.log(`Session ${storedId} is already imported; leaving it untouched`);
      return storedId;
    }

    if (options?.dryRun === true) {
      yield* Console.log(`Would import Cline session ${storedId} into storage`);
      return storedId;
    }

    // Sepia cannot mint Devin's cog scaffolding (model cog, tool allow-list,
    // profile prompt), so graft it from a sibling session in the store —
    // same working directory preferred — or at least fill the model cog.
    let cogsJson = session.cogsJson;
    const donors = yield* repo.list().pipe(Effect.orElseSucceed(() => [] as const));
    const donor =
      donors.find(
        (s) =>
          s.id !== session.id &&
          s.workingDirectory === session.workingDirectory &&
          modelCog(s.cogsJson),
      ) ?? donors.find((s) => s.id !== session.id && modelCog(s.cogsJson));
    if (donor) {
      cogsJson = donor.cogsJson;
      yield* Console.log(`Grafted cogs from session ${donor.id}`);
    } else {
      cogsJson = withModel(cogsJson, session.model);
      yield* Console.log("Warning: no donor session found to graft cogs from; resume may fail");
    }

    yield* repo.save(
      Session.make({
        id: session.id,
        title: session.title,
        workingDirectory: session.workingDirectory,
        backendType: session.backendType,
        agentMode: session.agentMode,
        model: session.model,
        createdAt: session.createdAt,
        lastActivityAt: session.lastActivityAt,
        mainChainId: session.mainChainId,
        shellLastSeenIndex: session.shellLastSeenIndex,
        cogsJson,
        workspaceDirs: session.workspaceDirs,
        hidden: session.hidden,
        metadata: session.metadata,
        nodes: session.nodes,
        promptHistory: session.promptHistory,
      }),
    );
    yield* Console.log(`Imported Cline session ${session.id} into storage`);
    return session.id;
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({ message: `Import failed: ${String(error)}`, cause: error }),
    ),
  );

export const exportCline = (
  sessionId: string,
  outDir: string,
  options?: { readonly force?: boolean; readonly dryRun?: boolean },
) =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const opt = yield* repo.getById(sessionId);
    if (Option.isNone(opt)) {
      return yield* Effect.fail(
        new ConversionError({ message: `Session not found: ${sessionId}`, cause: null }),
      );
    }
    yield* Cline.toDirectory(opt.value, outDir, options);
    yield* Console.log(`Exported session ${sessionId} to ${outDir}`);
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({ message: `Export failed: ${String(error)}`, cause: error }),
    ),
  );

/**
 * Export a Devin session straight into the Cline CLI store: artifacts land in
 * `<dataDir>/sessions/<session-id>/` and the session index is updated, so the
 * session shows up in `cline history` and resumes with `cline --id <session-id>`.
 */
export const installCline = (
  sessionId: string,
  dataDir: string,
  newSessionId?: string,
  options?: { readonly force?: boolean },
) =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const store = yield* ClineStore;

    const opt = yield* repo.getById(sessionId);
    if (Option.isNone(opt)) {
      return yield* Effect.fail(
        new ConversionError({ message: `Session not found: ${sessionId}`, cause: null }),
      );
    }

    const installed = yield* store.install(opt.value, newSessionId, options);
    yield* Console.log(`Installed session ${installed} into ${dataDir}`);
    yield* Console.log(`Resume it with: cline --id ${installed} -m <model>`);
    return installed;
  }).pipe(
    Effect.mapError((error) =>
      error instanceof ConversionError
        ? error
        : new ConversionError({ message: `Install failed: ${String(error)}`, cause: error }),
    ),
  );

export const listSessions = () =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const sessions = yield* repo.list();
    if (sessions.length === 0) {
      yield* Console.log("No sessions found");
    } else {
      for (const s of sessions) {
        yield* Console.log(`${s.id}\t${s.title}\t${s.workingDirectory}`);
      }
    }
  });
