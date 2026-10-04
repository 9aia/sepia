/**
 * Sidebar section configuration — which sections render, in what order,
 * under which label, and (for flat session lists) capped at how many rows.
 * Persisted inside `SepiaSettings.sidebar.sections`; array order is the
 * render order.
 */
export type SidebarSectionId = "pinned" | "projects" | "sessions" | "folders" | "archived";

export interface SidebarSectionConfig {
  readonly id: SidebarSectionId;
  /** Custom section label — undefined falls back to the default label. */
  readonly label?: string;
  /** false = section hidden from the sidebar. */
  readonly enabled: boolean;
  /** Row cap for flat sections — only meaningful where a default exists. */
  readonly limit?: number;
}

export const SIDEBAR_SECTION_IDS: ReadonlyArray<SidebarSectionId> = [
  "pinned",
  "projects",
  "sessions",
  "folders",
  "archived",
];

export const SIDEBAR_SECTION_LABELS: Record<SidebarSectionId, string> = {
  pinned: "Pinned",
  projects: "Projects",
  sessions: "Sessions",
  folders: "Folders",
  archived: "Archived",
};

/** Default row cap per section — undefined where the section isn't a flat list. */
export const SIDEBAR_SECTION_LIMITS: Record<SidebarSectionId, number | undefined> = {
  pinned: 5,
  projects: undefined,
  sessions: 8,
  folders: undefined,
  archived: 20,
};

export const defaultSidebarSections = (): SidebarSectionConfig[] =>
  SIDEBAR_SECTION_IDS.map((id) => ({ id, enabled: true, limit: SIDEBAR_SECTION_LIMITS[id] }));

const isSectionId = (value: unknown): value is SidebarSectionId =>
  typeof value === "string" && (SIDEBAR_SECTION_IDS as ReadonlyArray<string>).includes(value);

const validLimit = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.floor(value) : undefined;

/**
 * Reconcile a stored section list with the known sections: unknown ids and
 * duplicates drop, sections missing from storage append in default order
 * (forward-compat when a new section ships), array order is preserved.
 */
export const normalizeSidebarSections = (raw: unknown): SidebarSectionConfig[] => {
  const list = Array.isArray(raw) ? raw : [];
  const seen = new Set<SidebarSectionId>();
  const sections: SidebarSectionConfig[] = [];
  for (const item of list) {
    if (typeof item !== "object" || item === null) continue;
    const entry = item as Record<string, unknown>;
    if (!isSectionId(entry.id) || seen.has(entry.id)) continue;
    seen.add(entry.id);
    const limit = SIDEBAR_SECTION_LIMITS[entry.id];
    sections.push({
      id: entry.id,
      enabled: entry.enabled !== false,
      label: typeof entry.label === "string" && entry.label.trim() !== "" ? entry.label : undefined,
      limit: limit === undefined ? undefined : (validLimit(entry.limit) ?? limit),
    });
  }
  for (const section of defaultSidebarSections()) {
    if (!seen.has(section.id)) sections.push(section);
  }
  return sections;
};

export const sidebarSectionLabel = (section: SidebarSectionConfig): string =>
  section.label ?? SIDEBAR_SECTION_LABELS[section.id];

export const sidebarSectionLimit = (section: SidebarSectionConfig): number | undefined =>
  section.limit ?? SIDEBAR_SECTION_LIMITS[section.id];
