import type { HistoryMessage } from "./types";

/** A rule file declared in a `<rules>` block — `content` is the `<rule>` body. */
export interface ContextRule {
  readonly name: string;
  readonly path: string;
  /** The rule body verbatim — omitted when the `<rule>` tag is empty. */
  readonly content?: string;
}

/** A skill advertised via `<available_skills>` `- **name**: description (source: …)` lines. */
export interface ContextSkill {
  readonly name: string;
  readonly description?: string;
  /** The `(source: …)` suffix verbatim — usually a SKILL.md path or a builtin id. */
  readonly source?: string;
}

/** A background-subagent report lifted out of `<subagent_completion_notification>`. */
export interface ContextReport {
  /**
   * Derived label — the report's first line (`#` heading marks stripped).
   * Only present when the report runs past one line; the title line is
   * lifted out of `body`.
   */
  readonly title?: string;
  /** The report text — the bracketed `[…]` notification header is removed. */
  readonly body: string;
  /** `agent_id=…` from the `[Background subagent …]` header, when present. */
  readonly agentId?: string;
  /** The system node's `createdAt` (epoch ms), when the store recorded one. */
  readonly at?: number;
}

/** One `promptText` slice under a heading at the prompt's minimum heading depth. */
export interface PromptSection {
  readonly title: string;
  readonly body: string;
}

/** Context parsed out of agent-stored system nodes (devin's system_info + rules). */
export interface SystemContext {
  readonly workspaces: ReadonlyArray<string>;
  readonly platform: string | null;
  readonly osVersion: string | null;
  readonly date: string | null;
  readonly rules: ReadonlyArray<ContextRule>;
  /** Advertised skills — omitted entirely when no `<available_skills>` block parsed. */
  readonly skills?: ReadonlyArray<ContextSkill>;
  /** Background-subagent reports delivered as system notifications. */
  readonly reports: ReadonlyArray<ContextReport>;
  /** The rest of the system text — the agent's own prompt. */
  readonly promptText: string;
  /**
   * `promptText` pre-split on its shallowest markdown headings (devin's
   * `# Modes` / `# Style`) — additive; `promptText` stays whole.
   */
  readonly promptSections?: ReadonlyArray<PromptSection>;
}

const SYS_INFO_RE = /<system_info>([\s\S]*?)<\/system_info>/g;
const RULES_RE = /<rules[^>]*>([\s\S]*?)<\/rules>/g;
const RULE_RE = /<rule[^>]*?name="([^"]+)"[^>]*?path="([^"]+)"[^>]*?>([\s\S]*?)<\/rule>/g;
const SKILLS_RE = /<available_skills[^>]*>([\s\S]*?)<\/available_skills>/g;
const SKILL_LINE_RE = /^[ \t]*-[ \t]+\*\*([^*]+)\*\*:[ \t]*(.+?)[ \t]*$/;
const SKILL_SOURCE_RE = /[ \t]*\(source:[ \t]*(.+?)\)[ \t]*$/;
const SUBAGENT_RE =
  /<subagent_completion_notification>([\s\S]*?)<\/subagent_completion_notification>/g;
const REPORT_HEADER_RE = /^\[[^\]\n]*\][ \t]*(?:\n|$)/;
const AGENT_ID_RE = /agent_id=([^\s\]]+)/;
const FIELD_RE = /^(Platform|OS Version|Today's date):\s*(.+)$/;
const HEADING_RE = /^[ \t]{0,3}(#{1,6})[ \t]+(.+?)[ \t]*$/;
const FENCE_RE = /^[ \t]{0,3}(```+|~~~+)/;
const HEADING_MARKS_RE = /^#{1,6}[ \t]+/;

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

/**
 * One notification body → a report. The `[Background subagent with
 * agent_id=… completed]` header line comes off; the report's first line
 * becomes `title` when a body remains after it.
 */
const parseReport = (raw: string, createdAt: number): ContextReport | null => {
  let body = raw;
  let agentId: string | undefined;
  const header = REPORT_HEADER_RE.exec(body);
  if (header !== null) {
    agentId = AGENT_ID_RE.exec(header[0])?.[1];
    body = body.slice(header[0].length);
  }
  body = body.trim();
  if (body === "") return null;
  let title: string | undefined;
  const newline = body.indexOf("\n");
  if (newline !== -1) {
    const first = body.slice(0, newline).trim();
    // body.trim() guarantees text after the first newline.
    body = body.slice(newline + 1).trim();
    title = first.replace(HEADING_MARKS_RE, "").trim() || undefined;
  }
  return {
    ...(title !== undefined ? { title } : {}),
    body,
    ...(agentId !== undefined ? { agentId } : {}),
    ...(Number.isFinite(createdAt) && createdAt > 0 ? { at: createdAt } : {}),
  };
};

/**
 * `promptText` split into headed sections at the prompt's shallowest
 * heading depth — `# Modes`, `# Style`, … — with deeper headings left
 * inside each section body. Headings inside fenced code don't split.
 */
const splitPromptSections = (prompt: string): PromptSection[] | undefined => {
  const lines = prompt.split("\n");
  const heads: Array<{ index: number; depth: number; title: string }> = [];
  let fence: string | null = null;
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i] as string;
    const fenceMark = FENCE_RE.exec(line);
    if (fenceMark !== null) {
      const mark = (fenceMark[1] as string).slice(0, 3);
      fence = fence === null ? mark : fence === mark ? null : fence;
      continue;
    }
    if (fence !== null) continue;
    const heading = HEADING_RE.exec(line);
    if (heading !== null) {
      heads.push({
        index: i,
        depth: (heading[1] as string).length,
        title: (heading[2] as string).trim(),
      });
    }
  }
  if (heads.length === 0) return undefined;
  const minDepth = Math.min(...heads.map((head) => head.depth));
  const tops = heads.filter((head) => head.depth === minDepth);
  const sections: PromptSection[] = [];
  for (let i = 0; i < tops.length; i += 1) {
    const head = tops[i] as (typeof tops)[number];
    const end = i + 1 < tops.length ? (tops[i + 1] as (typeof tops)[number]).index : lines.length;
    const body = lines
      .slice(head.index + 1, end)
      .join("\n")
      .trim();
    if (body === "") continue;
    sections.push({ title: head.title, body });
  }
  return sections.length === 0 ? undefined : sections;
};

/** Folds system-role IR nodes into one display context (dupes collapse). */
export const parseSystemContext = (messages: ReadonlyArray<HistoryMessage>): SystemContext => {
  const workspaces: string[] = [];
  const fields = new Map<string, string>();
  const rules = new Map<string, ContextRule>();
  const skills = new Map<string, ContextSkill>();
  const reports: ContextReport[] = [];
  const seenReports = new Set<string>();
  const rest: string[] = [];
  for (const message of messages) {
    let text = message.content;
    text = text.replace(SUBAGENT_RE, (_match, body: string) => {
      const raw = body.trim();
      if (raw === "" || seenReports.has(raw)) return "";
      const report = parseReport(raw, message.createdAt);
      if (report !== null) {
        seenReports.add(raw);
        reports.push(report);
      }
      return "";
    });
    text = text.replace(SYS_INFO_RE, (_match, info: string) => {
      parseSystemInfo(info, { workspaces, fields });
      return "";
    });
    text = text.replace(RULES_RE, (_match, body: string) => {
      for (const rule of body.matchAll(RULE_RE)) {
        const name = rule[1] as string;
        const path = rule[2] as string;
        const content = (rule[3] as string).trim();
        const key = `${name}:${path}`;
        const existing = rules.get(key);
        if (existing !== undefined) {
          // A later block may carry the body the first occurrence lacked.
          if (existing.content === undefined && content !== "")
            rules.set(key, { name, path, content });
          continue;
        }
        rules.set(key, content === "" ? { name, path } : { name, path, content });
      }
      return "";
    });
    text = text.replace(SKILLS_RE, (_match, body: string) => {
      for (const line of body.split("\n")) {
        const skill = SKILL_LINE_RE.exec(line);
        if (skill === null) continue;
        const name = (skill[1] as string).trim();
        if (name === "" || skills.has(name)) continue;
        let description = (skill[2] as string).trim();
        let source: string | undefined;
        const src = SKILL_SOURCE_RE.exec(description);
        if (src !== null) {
          source = (src[1] as string).trim();
          description = description.slice(0, src.index).trim();
        }
        skills.set(name, {
          name,
          ...(description !== "" ? { description } : {}),
          ...(source !== undefined && source !== "" ? { source } : {}),
        });
      }
      return "";
    });
    const trimmed = text.trim();
    if (trimmed !== "") rest.push(trimmed);
  }
  const promptText = rest.join("\n\n");
  const promptSections = splitPromptSections(promptText);
  return {
    workspaces,
    platform: fields.get("Platform") ?? null,
    osVersion: fields.get("OS Version") ?? null,
    date: fields.get("Today's date") ?? null,
    rules: [...rules.values()],
    ...(skills.size > 0 ? { skills: [...skills.values()] } : {}),
    reports,
    promptText,
    ...(promptSections !== undefined ? { promptSections } : {}),
  };
};
