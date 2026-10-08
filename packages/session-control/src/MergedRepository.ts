import { Effect, Option } from "effect";
import type { Session, SessionNodeWindow, SessionRepositoryService } from "sepia-core";

/**
 * Maps a store backend to the agent that can resume it. `"cursor"` maps to
 * itself even though no ACP runtime exists — keeping the id distinct lets
 * the control plane refuse attach instead of grafting the session onto the
 * default agent.
 */
export const agentForBackend = (backendType: string): string =>
  backendType === "cline" || backendType === "claude" || backendType === "cursor"
    ? backendType
    : "devin";

/**
 * Overlays one primary repository (Devin's store) with extra read sources
 * (e.g. Cline's session dirs). Extra-repo failures degrade to "empty / not
 * found" so a broken overlay can't take down the primary listing; primary
 * errors propagate as before.
 */
export const mergeRepositories = (
  primary: SessionRepositoryService,
  extras: ReadonlyArray<SessionRepositoryService>,
): SessionRepositoryService => ({
  list: () =>
    Effect.all([
      primary.list(),
      ...extras.map((repo) =>
        repo.list().pipe(Effect.catchAll(() => Effect.succeed([] as ReadonlyArray<Session>))),
      ),
    ]).pipe(
      // Primary wins on id collisions so a session present in two stores
      // keeps the primary backend's metadata.
      Effect.map((groups) => {
        const seen = new Set<string>();
        return groups
          .flat()
          .filter((session) => (seen.has(session.id) ? false : (seen.add(session.id), true)))
          .sort((a, b) => b.lastActivityAt - a.lastActivityAt);
      }),
    ),

  // `agentId` narrows the lookup to sessions whose backend maps to that
  // agent — ids collide across stores, so the primary's copy is only
  // correct when the caller asked for its agent.
  getById: (id, agentId) =>
    Effect.gen(function* () {
      const forAgent = (session: Session): boolean =>
        agentId === undefined || agentForBackend(session.backendType) === agentId;
      const found = yield* primary.getById(id);
      if (Option.isSome(found) && forAgent(found.value)) return found;
      for (const repo of extras) {
        const hit = yield* repo
          .getById(id)
          .pipe(Effect.catchAll(() => Effect.succeed(Option.none<Session>())));
        if (Option.isSome(hit) && forAgent(hit.value)) return hit;
      }
      return Option.none<Session>();
    }),

  // Same fan-out as `getById`; repos without `summary` fall back to it.
  summary: (id, agentId) =>
    Effect.gen(function* () {
      const forAgent = (session: Session): boolean =>
        agentId === undefined || agentForBackend(session.backendType) === agentId;
      const read = (repo: SessionRepositoryService) =>
        repo.summary !== undefined ? repo.summary(id) : repo.getById(id);
      const found = yield* read(primary);
      if (Option.isSome(found) && forAgent(found.value)) return found;
      for (const repo of extras) {
        const hit = yield* read(repo).pipe(
          Effect.catchAll(() => Effect.succeed(Option.none<Session>())),
        );
        if (Option.isSome(hit) && forAgent(hit.value)) return hit;
      }
      return Option.none<Session>();
    }),

  hasSession: (id) =>
    Effect.gen(function* () {
      if (yield* primary.hasSession(id)) return true;
      for (const repo of extras) {
        const hit = yield* repo.hasSession(id).pipe(Effect.catchAll(() => Effect.succeed(false)));
        if (hit) return true;
      }
      return false;
    }),

  // Writes stay scoped to the primary store; overlays are read-only.
  save: primary.save,
  delete: primary.delete,

  /**
   * Paged history delegated to whichever store owns the id — repos without a
   * native window are skipped entirely (the caller's `getById` fallback covers
   * them). Extra-repo failures degrade to a miss, same as `getById`.
   */
  nodesWindow: (id, options) =>
    Effect.gen(function* () {
      const forAgent = (backendType: string): boolean =>
        options.agentId === undefined || agentForBackend(backendType) === options.agentId;
      // Primary errors propagate (same as getById); extras degrade to a miss.
      if (primary.nodesWindow !== undefined) {
        const hit = yield* primary.nodesWindow(id, options);
        if (Option.isSome(hit) && forAgent(hit.value.backendType)) return hit;
      }
      for (const repo of extras) {
        if (repo.nodesWindow === undefined) continue;
        const hit = yield* repo
          .nodesWindow(id, options)
          .pipe(Effect.catchAll(() => Effect.succeed(Option.none<SessionNodeWindow>())));
        if (Option.isSome(hit) && forAgent(hit.value.backendType)) return hit;
      }
      return Option.none<SessionNodeWindow>();
    }),
});
