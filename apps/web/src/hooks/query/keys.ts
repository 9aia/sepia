export const queryKeys = {
  sessions: ["sessions"] as const,
  projects: ["projects"] as const,
  config: ["config"] as const,
  agents: ["agents"] as const,
  user: ["user"] as const,
  node: ["node"] as const,
  servers: ["servers"] as const,
  dirs: (path: string) => ["dirs", path] as const,
  /**
   * History is per owning node: federated keys prepend `node` so the same
   * `agent:id` on two machines can't share a cache entry. Single-node keys
   * keep the legacy `agent:id`/`id` shape.
   */
  history: (sessionId: string, agent?: string, node?: string) =>
    [
      "history",
      node === undefined || node === ""
        ? agent === undefined || agent === ""
          ? sessionId
          : `${agent}:${sessionId}`
        : `${node}:${agent ?? ""}:${sessionId}`,
    ] as const,
  /**
   * The held-session lock probe (`GET /api/sessions?withLocks=1` filtered to
   * one session) while the panel reads read-only. `[node, agent, id]`
   * segments so a `session` feed event can refetch just the watched row.
   */
  heldSession: (node: string, agent: string, sessionId: string) =>
    ["held-session", node, agent, sessionId] as const,
  /** Checkpoint refs a session recorded — same node-scoped keying as history. */
  checkpoints: (sessionId: string, agent?: string, node?: string) =>
    [
      "checkpoints",
      node === undefined || node === ""
        ? agent === undefined || agent === ""
          ? sessionId
          : `${agent}:${sessionId}`
        : `${node}:${agent ?? ""}:${sessionId}`,
    ] as const,
};
