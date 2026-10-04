import { Context, Effect, Option } from "effect";
import { Session, StorageError } from "./Domain.js";

export interface SessionRepositoryService {
  readonly save: (session: Session) => Effect.Effect<void, StorageError>;
  /**
   * Single-store implementations ignore `agentId` — merged repositories use
   * it to resolve an id that may collide across agents to the requested
   * agent's copy.
   */
  readonly getById: (
    id: string,
    agentId?: string,
  ) => Effect.Effect<Option.Option<Session>, StorageError>;
  readonly list: () => Effect.Effect<ReadonlyArray<Session>, StorageError>;
  readonly delete: (id: string) => Effect.Effect<void, StorageError>;
  readonly hasSession: (id: string) => Effect.Effect<boolean, StorageError>;
}

export class SessionRepository extends Context.Tag("SessionRepository")<
  SessionRepository,
  SessionRepositoryService
>() {}

/** The tables sepia reads and writes; an existing Devin store already has them. */
export const REQUIRED_TABLES = ["sessions", "message_nodes", "prompt_history"] as const;

/** A store that already carries the schema must not be migrated again. */
export const needsMigration = (tableNames: ReadonlySet<string>): boolean =>
  !REQUIRED_TABLES.every((table) => tableNames.has(table));
