import { Context, Effect, Option } from "effect";
import { type MessageNode, Session, StorageError } from "./Domain.js";

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
  /**
   * Paged history read — adapters that can page natively implement it.
   * `Option.none` means "session unknown to this store" (same contract as
   * `getById`); absent entirely means the adapter falls back to `getById`.
   */
  readonly nodesWindow?: (
    id: string,
    options: { readonly limit?: number; readonly before?: number; readonly agentId?: string },
  ) => Effect.Effect<Option.Option<SessionNodeWindow>, StorageError>;
  readonly delete: (id: string) => Effect.Effect<void, StorageError>;
  readonly hasSession: (id: string) => Effect.Effect<boolean, StorageError>;
}

/**
 * A paged node window an adapter can serve without parsing a session's whole
 * backlog. `nodes` is the window `start..start+len`; `toolCallNodes` carries
 * the nodes whose `toolCalls` the window's tool rows reference (the calls
 * themselves may live outside the window); `total` is the full node count.
 */
export interface SessionNodeWindow {
  readonly nodes: ReadonlyArray<MessageNode>;
  readonly toolCallNodes: ReadonlyArray<MessageNode>;
  readonly total: number;
  readonly start: number;
  /** Store backend — merged repos use it for `agentId` narrowing. */
  readonly backendType: string;
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
