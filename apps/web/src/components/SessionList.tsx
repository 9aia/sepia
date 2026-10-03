import { useMemo, useRef, useState, type FormEvent } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { useHotkey } from "@tanstack/react-hotkeys";
import type { CreateSessionInput, SessionSummary } from "../lib/types";

interface SessionListProps {
  sessions: SessionSummary[];
  selectedId: string | null;
  loading: boolean;
  error: string | null;
  creating: boolean;
  createError: string | null;
  onSelect: (id: string) => void;
  onCreate: (input: CreateSessionInput) => void;
  onDelete: (id: string) => void;
}

function formatUpdated(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const diffMinutes = Math.round((Date.now() - then) / 60000);
  if (diffMinutes < 1) return "just now";
  if (diffMinutes < 60) return `${diffMinutes}m ago`;
  const hours = Math.round(diffMinutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

const inFormField = (): boolean => {
  const el = document.activeElement;
  return (
    el instanceof HTMLInputElement ||
    el instanceof HTMLTextAreaElement ||
    el instanceof HTMLSelectElement
  );
};

export function SessionList({
  sessions,
  selectedId,
  loading,
  error,
  creating,
  createError,
  onSelect,
  onCreate,
  onDelete,
}: SessionListProps) {
  const [cwd, setCwd] = useState("");
  const [title, setTitle] = useState("");
  const [filter, setFilter] = useState("");
  const [debouncedFilter] = useDebouncedValue(filter, { wait: 200 });
  const asideRef = useRef<HTMLElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const filterRef = useRef<HTMLInputElement | null>(null);
  const cwdRef = useRef<HTMLInputElement | null>(null);

  const filtered = useMemo(() => {
    const needle = debouncedFilter.trim().toLowerCase();
    if (needle === "") return sessions;
    return sessions.filter((session) =>
      [session.title, session.cwd, session.id].some((field) =>
        field.toLowerCase().includes(needle),
      ),
    );
  }, [sessions, debouncedFilter]);

  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 76,
    overscan: 8,
  });

  const selectedIndex = filtered.findIndex((session) => session.id === selectedId);

  const selectByIndex = (index: number): void => {
    const session = filtered[index];
    if (session === undefined) return;
    virtualizer.scrollToIndex(index, { align: "auto" });
    onSelect(session.id);
  };

  useHotkey("Mod+K", () => filterRef.current?.focus(), { preventDefault: true });
  useHotkey("N", () => {
    if (inFormField()) return;
    cwdRef.current?.focus();
  });
  // ignoreInputs: false so arrows still navigate while the filter input is focused.
  useHotkey(
    "ArrowDown",
    () => {
      const next = selectedIndex === -1 ? 0 : Math.min(selectedIndex + 1, filtered.length - 1);
      selectByIndex(next);
    },
    { target: asideRef, preventDefault: true, ignoreInputs: false },
  );
  useHotkey(
    "ArrowUp",
    () => {
      const next = selectedIndex === -1 ? filtered.length - 1 : Math.max(selectedIndex - 1, 0);
      selectByIndex(next);
    },
    { target: asideRef, preventDefault: true, ignoreInputs: false },
  );
  useHotkey(
    "Escape",
    () => {
      setFilter("");
      filterRef.current?.blur();
    },
    { target: filterRef },
  );

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    const trimmedCwd = cwd.trim();
    if (trimmedCwd === "") return;
    const trimmedTitle = title.trim();
    onCreate({ cwd: trimmedCwd, title: trimmedTitle === "" ? undefined : trimmedTitle });
    setCwd("");
    setTitle("");
  };

  return (
    <aside className="session-list" ref={asideRef}>
      <header className="session-list__header">
        <h1 className="session-list__title">sepia</h1>
        <span className="session-list__count">{sessions.length}</span>
      </header>

      <form className="session-list__new" onSubmit={submit}>
        <input
          className="session-list__input"
          type="text"
          placeholder="Working directory (absolute)"
          aria-label="Working directory"
          ref={cwdRef}
          value={cwd}
          onChange={(event) => setCwd(event.target.value)}
        />
        <input
          className="session-list__input"
          type="text"
          placeholder="Title (optional)"
          aria-label="Session title"
          value={title}
          onChange={(event) => setTitle(event.target.value)}
        />
        <button
          className="session-list__create"
          type="submit"
          disabled={creating || cwd.trim() === ""}
        >
          {creating ? "Creating…" : "New session"}
        </button>
        {createError && (
          <p className="session-list__status session-list__status--error">{createError}</p>
        )}
      </form>

      <input
        className="session-list__filter"
        type="search"
        placeholder="Filter sessions… (Ctrl/⌘+K)"
        aria-label="Filter sessions"
        ref={filterRef}
        value={filter}
        onChange={(event) => setFilter(event.target.value)}
      />

      {loading && <p className="session-list__status">Loading sessions…</p>}
      {error && <p className="session-list__status session-list__status--error">{error}</p>}
      {!loading && !error && filtered.length === 0 && (
        <p className="session-list__status">
          {sessions.length === 0 ? "No sessions found." : "No sessions match the filter."}
        </p>
      )}

      <div
        className="session-list__items"
        ref={listRef}
        role="listbox"
        aria-label="Sessions"
        aria-activedescendant={selectedId === null ? undefined : `session-option-${selectedId}`}
      >
        <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}>
          {virtualizer.getVirtualItems().map((row) => {
            const session = filtered[row.index];
            if (session === undefined) return null;
            const selected = session.id === selectedId;
            return (
              <div
                key={session.id}
                id={`session-option-${session.id}`}
                role="option"
                aria-selected={selected}
                data-index={row.index}
                ref={virtualizer.measureElement}
                className="session-row"
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${row.start}px)`,
                }}
              >
                <button
                  type="button"
                  className={"session-item" + (selected ? " session-item--selected" : "")}
                  onClick={() => onSelect(session.id)}
                >
                  <div className="session-item__top">
                    <span className="session-item__title">{session.title}</span>
                    <span className={`badge badge--${session.agent}`}>{session.agent}</span>
                  </div>
                  <div className="session-item__cwd" title={session.cwd}>
                    {session.cwd}
                  </div>
                  <div className="session-item__meta">
                    <span>{formatUpdated(session.updatedAt)}</span>
                    {session.locked && (
                      <span
                        className="badge badge--locked"
                        title={`Locked by pid ${session.lockHolderPid ?? "unknown"}`}
                      >
                        locked
                      </span>
                    )}
                  </div>
                </button>
                <button
                  type="button"
                  className="session-item__delete"
                  aria-label={`Delete session ${session.title}`}
                  title="Delete session"
                  onClick={(event) => {
                    event.stopPropagation();
                    if (window.confirm(`Delete session "${session.title}"?`)) {
                      onDelete(session.id);
                    }
                  }}
                >
                  ×
                </button>
              </div>
            );
          })}
        </div>
      </div>
    </aside>
  );
}
