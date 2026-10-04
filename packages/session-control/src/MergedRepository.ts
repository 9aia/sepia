import { Effect, Option } from "effect";
import type { Session, SessionRepositoryService } from "sepia-core";

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

  getById: (id) =>
    Effect.gen(function* () {
      const found = yield* primary.getById(id);
      if (Option.isSome(found)) return found;
      for (const repo of extras) {
        const hit = yield* repo
          .getById(id)
          .pipe(Effect.catchAll(() => Effect.succeed(Option.none<Session>())));
        if (Option.isSome(hit)) return hit;
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
});
