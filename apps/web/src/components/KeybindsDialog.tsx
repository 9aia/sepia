import { useEffect, useState } from "react";
import { useStore } from "@tanstack/react-store";
import { sepiaStore, setKeybindsOpen } from "../lib/store";
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
  const [modKey, setModKey] = useState("Ctrl");
  useEffect(() => {
    if (navigator.platform.toUpperCase().includes("MAC")) setModKey("⌘");
  }, []);

  return (
    <Dialog open={open} onOpenChange={setKeybindsOpen}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
          <DialogDescription>Available anywhere in the app.</DialogDescription>
        </DialogHeader>
        <div className="text-xs font-medium text-muted-foreground uppercase">Sessions</div>
        <div className="divide-y divide-border/50">
          <Row keys={["↑", "↓"]} action="Navigate sessions" />
          <Row keys={["←", "→"]} action="Collapse / expand group" />
          <Row keys={["N"]} action="New session" />
          <Row keys={[modKey, "K"]} action="Focus filter" />
          <Row keys={["Esc"]} action="Clear filter" />
        </div>
        <div className="mt-3 text-xs font-medium text-muted-foreground uppercase">App</div>
        <div className="divide-y divide-border/50">
          <Row keys={[modKey, "B"]} action="Toggle sidebar" />
          <Row keys={["Shift", "/"]} action="This dialog" />
        </div>
      </DialogContent>
    </Dialog>
  );
}
