import { useState } from "react";
import {
  ArrowUp01Icon,
  KeyboardIcon,
  ProfileIcon,
  Settings02Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useUserInfo } from "../../hooks/query/useUserInfo";
import { setKeybindsOpen } from "../../lib/store";
import { ProfileDialog } from "../ProfileDialog";
import { SettingsDialog } from "../SettingsDialog";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";

export function UserProfile() {
  const { data: user } = useUserInfo();
  const [dialog, setDialog] = useState<"profile" | "settings" | null>(null);
  const username = user?.username ?? "…";
  const initial = username === "…" ? "?" : username.charAt(0).toUpperCase();

  return (
    <div className="border-t border-border p-2">
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              variant="ghost"
              className="h-auto w-full justify-start gap-2.5 px-2 py-1.5"
              aria-label="Account menu"
            />
          }
        >
          <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-secondary text-xs font-semibold text-secondary-foreground">
            {initial}
          </span>
          <span className="min-w-0 flex-1 text-left">
            <span className="block truncate text-sm font-medium">{username}</span>
            <span className="block truncate text-xs text-muted-foreground">
              {user?.hostname ?? ""}
            </span>
          </span>
          <HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2} className="text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          <DropdownMenuItem onClick={() => setDialog("profile")}>
            <HugeiconsIcon icon={ProfileIcon} strokeWidth={2} />
            Profile
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => setDialog("settings")}>
            <HugeiconsIcon icon={Settings02Icon} strokeWidth={2} />
            Settings
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => setKeybindsOpen(true)}>
            <HugeiconsIcon icon={KeyboardIcon} strokeWidth={2} />
            Keyboard shortcuts
            <DropdownMenuShortcut>?</DropdownMenuShortcut>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <ProfileDialog open={dialog === "profile"} onOpenChange={(o) => !o && setDialog(null)} />
      <SettingsDialog open={dialog === "settings"} onOpenChange={(o) => !o && setDialog(null)} />
    </div>
  );
}
