import { useState } from "react";
import { useStore } from "@tanstack/react-store";
import { useHasKeyboard } from "../../lib/keyboard";
import {
  ArrowUp01Icon,
  KeyboardIcon,
  ProfileIcon,
  Settings02Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useUserInfo } from "../../hooks/query/useUserInfo";
import { useHealth } from "../../hooks/query/useHealth";
import { useAppHotkey } from "../../lib/keybinds";
import { sepiaStore, setSettingsOpen } from "../../lib/store";
import { ProfileDialog } from "../ProfileDialog";
import { SettingsDialog } from "../SettingsDialog";
import { Button } from "../ui/button";
import { Skeleton } from "../ui/skeleton";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";

export function UserProfile() {
  const hasKeyboard = useHasKeyboard();
  const { data: user, isLoading, isError } = useUserInfo();
  const health = useHealth();
  const [dialog, setDialog] = useState<"profile" | null>(null);
  const settingsOpen = useStore(sepiaStore, (state) => state.settingsOpen);
  useAppHotkey("app.settings", () => setSettingsOpen(!settingsOpen));
  const openSettings = (open: boolean): void => setSettingsOpen(open);
  const username = user?.username ?? (isError ? "Unavailable" : "");
  const initial = user?.username.charAt(0).toUpperCase();

  return (
    <div className="border-t border-border p-2 pb-[max(0.5rem,env(safe-area-inset-bottom))]">
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
          {isLoading ? (
            <Skeleton className="size-8 shrink-0 rounded-full" />
          ) : (
            <span className="relative shrink-0">
              <span className="flex size-8 items-center justify-center rounded-full bg-secondary text-xs font-semibold text-secondary-foreground">
                {initial !== undefined ? (
                  initial
                ) : (
                  <HugeiconsIcon icon={ProfileIcon} strokeWidth={2} />
                )}
              </span>
              <span
                role="status"
                aria-label={health.isError ? "Server unreachable" : "Server online"}
                title={health.isError ? "Server unreachable" : "Server online"}
                className={`absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-sidebar ${
                  health.isError ? "animate-pulse bg-destructive" : "bg-emerald-500"
                }`}
              />
            </span>
          )}
          <span className="min-w-0 flex-1 text-left">
            {isLoading ? (
              <span className="block space-y-1.5">
                <Skeleton className="h-3.5 w-2/3" />
                <Skeleton className="h-3 w-1/3" />
              </span>
            ) : (
              <>
                <span className="block truncate text-sm font-medium">{username}</span>
                <span className="block truncate text-xs text-muted-foreground">
                  {isError ? "Couldn't load profile" : (user?.hostname ?? "")}
                </span>
              </>
            )}
          </span>
          <HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2} className="text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          <DropdownMenuItem onClick={() => setDialog("profile")}>
            <HugeiconsIcon icon={ProfileIcon} strokeWidth={2} />
            Profile
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => setSettingsOpen(true)}>
            <HugeiconsIcon icon={Settings02Icon} strokeWidth={2} />
            Settings
          </DropdownMenuItem>
          {hasKeyboard && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={() => setSettingsOpen(true, "keyboard")}>
                <HugeiconsIcon icon={KeyboardIcon} strokeWidth={2} />
                Keyboard shortcuts
                <DropdownMenuShortcut>?</DropdownMenuShortcut>
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <ProfileDialog open={dialog === "profile"} onOpenChange={(o) => !o && setDialog(null)} />
      <SettingsDialog open={settingsOpen} onOpenChange={openSettings} />
    </div>
  );
}
