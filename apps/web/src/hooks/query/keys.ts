export const queryKeys = {
  sessions: ["sessions"] as const,
  projects: ["projects"] as const,
  config: ["config"] as const,
  agents: ["agents"] as const,
  user: ["user"] as const,
  dirs: (path: string) => ["dirs", path] as const,
  history: (sessionId: string) => ["history", sessionId] as const,
};
