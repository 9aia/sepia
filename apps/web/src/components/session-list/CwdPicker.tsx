import { useState } from "react";
import { FolderOpenIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useDirs } from "../../hooks/query/useDirs";
import { useUserInfo } from "../../hooks/query/useUserInfo";
import { Button } from "../ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "../ui/dialog";
import { Input } from "../ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "../ui/select";

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
 * Pick from session dirs or type a custom absolute path.
 */
export function CwdPicker({ value, dirs, onChange }: CwdPickerProps) {
  const [customOpen, setCustomOpen] = useState(false);
  const [custom, setCustom] = useState(value);
  const { data: suggestions = [] } = useDirs(parentOf(custom));
  const { data: user } = useUserInfo();
  const customDirs = [
    ...new Set([user?.homedir, ...suggestions].filter((d): d is string => d !== undefined)),
  ];

  return (
    <>
      <Select
        value={value}
        onValueChange={(v) => {
          if (v === "__custom__") {
            setCustom(value);
            setCustomOpen(true);
            return;
          }
          if (v !== null) onChange(v);
        }}
      >
        <SelectTrigger
          aria-label="Working directory"
          title={`New sessions spawn in ${value}`}
          className="h-8 w-full gap-1.5 text-xs"
        >
          <HugeiconsIcon
            icon={FolderOpenIcon}
            strokeWidth={2}
            className="shrink-0 text-muted-foreground"
          />
          <span className="min-w-0 flex-1 truncate text-left">{shortName(value)}</span>
        </SelectTrigger>
        <SelectContent>
          {dirs.map((dir) => (
            <SelectItem key={dir} value={dir} title={dir}>
              {shortName(dir)}
            </SelectItem>
          ))}
          <SelectItem value="__custom__">Custom directory…</SelectItem>
        </SelectContent>
      </Select>
      <Dialog open={customOpen} onOpenChange={setCustomOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Working directory</DialogTitle>
            <DialogDescription>
              Absolute path new sessions spawn in — subdirs appear as you type.
            </DialogDescription>
          </DialogHeader>
          <Input
            autoFocus
            value={custom}
            onChange={(event) => setCustom(event.target.value)}
            list="custom-cwd-dirs"
            placeholder="/home/you/projects/app"
          />
          <datalist id="custom-cwd-dirs">
            {customDirs.map((dir) => (
              <option key={dir} value={dir} />
            ))}
          </datalist>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setCustomOpen(false)}>
              Cancel
            </Button>
            <Button
              onClick={() => {
                const trimmed = custom.trim();
                if (trimmed === "") return;
                onChange(trimmed);
                setCustomOpen(false);
              }}
            >
              Set directory
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
