import { useMemo, useState } from "react";
import { FolderOpenIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useDirs } from "../../hooks/query/useDirs";
import { useUserInfo } from "../../hooks/query/useUserInfo";
import {
  Autocomplete,
  AutocompleteEmpty,
  AutocompleteInput,
  AutocompleteItem,
  AutocompleteList,
  AutocompletePopup,
  AutocompletePortal,
  AutocompletePositioner,
} from "../ui/autocomplete";

/** Last two path segments (`9aia/sepia`) — disambiguates same-named dirs. */
const shortName = (path: string): string => {
  const trimmed = path.replace(/\/+$/, "");
  const parts = trimmed.split("/").filter((p) => p !== "");
  return parts.slice(-2).join("/");
};

/** Parent dir of the value being typed; null when it can't be absolute. */
const parentOf = (value: string): string | null => {
  const idx = value.replace(/\/+$/, "").lastIndexOf("/");
  if (idx === -1) return null;
  return idx === 0 ? "/" : value.slice(0, idx);
};

interface CwdPickerProps {
  readonly value: string;
  /** Candidate directories — unique session cwds. */
  readonly dirs: ReadonlyArray<string>;
  onChange: (cwd: string) => void;
}

/**
 * The sidebar's working-directory context — the dir new sessions spawn in.
 * Type to filter known session dirs and live subdirs of the typed parent;
 * press Enter on a free-form absolute path to use it.
 */
export function CwdPicker({ value, dirs, onChange }: CwdPickerProps) {
  const [input, setInput] = useState(value);
  const { data: suggested = [] } = useDirs(parentOf(input));
  const { data: user } = useUserInfo();

  const items = useMemo(
    () => [
      ...new Set(
        [...dirs, ...suggested, user?.homedir].filter((d): d is string => d !== undefined),
      ),
    ],
    [dirs, suggested, user?.homedir],
  );

  const commit = (next: string): void => {
    const trimmed = next.trim();
    if (trimmed !== "") onChange(trimmed);
  };

  return (
    <Autocomplete
      items={items}
      value={input}
      onValueChange={(next) => {
        setInput(next);
        // An item press fills the input with the item's path — commit it.
        if (items.includes(next)) commit(next);
      }}
    >
      <div className="relative">
        <AutocompleteInput
          aria-label="Working directory"
          title={`New sessions spawn in ${value}`}
          placeholder="/home/you/projects/app"
          className="h-8 w-full rounded-3xl pl-9 text-xs"
          onKeyDown={(event) => {
            if (event.key === "Enter") commit(input);
          }}
        />
        <HugeiconsIcon
          icon={FolderOpenIcon}
          strokeWidth={2}
          className="pointer-events-none absolute top-1/2 left-3 size-3.5 -translate-y-1/2 text-muted-foreground"
        />
      </div>
      <AutocompletePortal>
        <AutocompletePositioner>
          <AutocompletePopup>
            <AutocompleteEmpty>No matching directories.</AutocompleteEmpty>
            <AutocompleteList>
              {(dir: string) => (
                <AutocompleteItem key={dir} value={dir}>
                  <span className="min-w-0 flex-1 truncate">{shortName(dir)}</span>
                  <span className="shrink-0 truncate text-muted-foreground/60">{dir}</span>
                </AutocompleteItem>
              )}
            </AutocompleteList>
          </AutocompletePopup>
        </AutocompletePositioner>
      </AutocompletePortal>
    </Autocomplete>
  );
}
