import { useEffect, useState } from "react";
import { useStore } from "@tanstack/react-store";
import { sepiaStore, setKeybindsOpen } from "../lib/store";
import { settingsStore } from "../lib/settings";
import { KEYBINDS, formatKey, resolveKey } from "../lib/keybinds";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import { Kbd } from "./ui/kbd";

function Row({ keys, action }: { readonly keys: string[]; readonly action: string }) {
  return (
    <div className="flex items-center justify-between py-1.5 text-sm">
      <span>{action}</span>
      <span className="flex gap-1">
        {keys.map((key) => (
          <Kbd key={key}>{key}</Kbd>
        ))}
      </span>
    </div>
  );
}

export function KeybindsDialog() {
  const open = useStore(sepiaStore, (state) => state.keybindsOpen);
  const settings = useStore(settingsStore);
  const groups = [...new Set(KEYBINDS.map((keybind) => keybind.group))];
  const [modKey, setModKey] = useState("Ctrl");
  useEffect(() => {
    if (navigator.platform.toUpperCase().includes("MAC")) setModKey("⌘");
  }, []);

  return (
    <Dialog open={open} onOpenChange={setKeybindsOpen}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
          <DialogDescription>Available anywhere in the app.</DialogDescription>
        </DialogHeader>
        {groups.map((group) => (
          <div key={group}>
            <div className="text-xs font-medium text-muted-foreground uppercase">{group}</div>
            <div className="divide-y divide-border/50">
              {KEYBINDS.filter((keybind) => keybind.group === group).map((keybind) => {
                const key = resolveKey(settings, keybind.id);
                return (
                  <Row
                    key={keybind.id}
                    keys={key === null ? [] : formatKey(key, modKey)}
                    action={key === null ? `${keybind.label} (disabled)` : keybind.label}
                  />
                );
              })}
            </div>
          </div>
        ))}
      </DialogContent>
    </Dialog>
  );
}
