export const queryKeys = {
  sessions: ["sessions"] as const,
  agents: ["agents"] as const,
  user: ["user"] as const,
  history: (sessionId: string) => ["history", sessionId] as const,
};
