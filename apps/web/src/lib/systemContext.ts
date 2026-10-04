import type { HistoryMessage } from "./types";

/** Context parsed out of agent-stored system nodes (devin's system_info + rules). */
export interface SystemContext {
  readonly workspaces: ReadonlyArray<string>;
  readonly platform: string | null;
  readonly osVersion: string | null;
  readonly date: string | null;
  readonly rules: ReadonlyArray<{ readonly name: string; readonly path: string }>;
  /** Background-subagent reports delivered as system notifications. */
  readonly reports: ReadonlyArray<string>;
  /** The rest of the system text — the agent's own prompt. */
  readonly promptText: string;
}

const SYS_INFO_RE = /<system_info>([\s\S]*?)<\/system_info>/g;
const RULES_RE = /<rules[^>]*>([\s\S]*?)<\/rules>/g;
const RULE_RE = /<rule[^>]*?name="([^"]+)"[^>]*?path="([^"]+)"[^>]*>/g;
const SUBAGENT_RE =
  /<subagent_completion_notification>([\s\S]*?)<\/subagent_completion_notification>/g;
const FIELD_RE = /^(Platform|OS Version|Today's date):\s*(.+)$/;

const parseSystemInfo = (
  info: string,
  ctx: { workspaces: string[]; fields: Map<string, string> },
): void => {
  const lines = info.split("\n");
  let inWorkspaces = false;
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith("Current workspace directories")) {
      inWorkspaces = true;
      continue;
    }
    if (inWorkspaces && trimmed !== "" && line.startsWith("  ")) {
      const cwd = trimmed.replace(/\s*\(cwd\)\s*$/, "");
      if (!ctx.workspaces.includes(cwd)) ctx.workspaces.push(cwd);
      continue;
    }
    inWorkspaces = false;
    const field = FIELD_RE.exec(trimmed);
    if (field) ctx.fields.set(field[1] as string, (field[2] as string).trim());
  }
};

/** Folds system-role IR nodes into one display context (dupes collapse). */
export const parseSystemContext = (messages: ReadonlyArray<HistoryMessage>): SystemContext => {
  const workspaces: string[] = [];
  const fields = new Map<string, string>();
  const rules: Array<{ name: string; path: string }> = [];
  const reports: string[] = [];
  const rest: string[] = [];
  for (const message of messages) {
    let text = message.content;
    text = text.replace(SUBAGENT_RE, (_match, body: string) => {
      const report = body.trim();
      if (report !== "" && !reports.includes(report)) reports.push(report);
      return "";
    });
    text = text.replace(SYS_INFO_RE, (_match, info: string) => {
      parseSystemInfo(info, { workspaces, fields });
      return "";
    });
    text = text.replace(RULES_RE, (_match, body: string) => {
      const seen = new Set(rules.map((r) => `${r.name}:${r.path}`));
      for (const rule of body.matchAll(RULE_RE)) {
        const name = rule[1] as string;
        const path = rule[2] as string;
        if (!seen.has(`${name}:${path}`)) rules.push({ name, path });
      }
      return "";
    });
    const trimmed = text.trim();
    if (trimmed !== "") rest.push(trimmed);
  }
  return {
    workspaces,
    platform: fields.get("Platform") ?? null,
    osVersion: fields.get("OS Version") ?? null,
    date: fields.get("Today's date") ?? null,
    rules,
    reports,
    promptText: rest.join("\n\n"),
  };
};
