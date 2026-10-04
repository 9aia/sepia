export function formatUpdated(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const diffMinutes = Math.round((Date.now() - then) / 60000);
  if (diffMinutes < 1) return "just now";
  if (diffMinutes < 60) return `${diffMinutes}m ago`;
  const hours = Math.round(diffMinutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

export const projectName = (cwd: string): string => {
  const trimmed = cwd.replace(/\/+$/, "");
  const last = trimmed.split("/").pop();
  return last === undefined || last === "" ? cwd : last;
};

/**
 * Agent-scoped session key — ids collide across agents (devin and cline mint
 * their own), so URLs/selection use `<agent>:<id>`.
 */
export const sessionKey = (session: { readonly agent: string; readonly id: string }): string =>
  `${session.agent}:${session.id}`;

/** Finds a session by its scoped key; bare ids (old links) match by id only. */
export const resolveSession = <T extends { readonly agent: string; readonly id: string }>(
  sessions: ReadonlyArray<T>,
  key: string | null | undefined,
): T | undefined => {
  if (key === null || key === undefined || key === "") return undefined;
  return (
    sessions.find((s) => sessionKey(s) === key) ??
    (key.includes(":") ? undefined : sessions.find((s) => s.id === key))
  );
};
