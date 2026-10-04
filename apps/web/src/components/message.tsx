import { useState, type ComponentProps } from "react";
import { CheckIcon, Copy01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { cn } from "cn";
import { Button } from "./ui/button";

/**
 * ReUI-style message row: avatar + content column, with header (time) and
 * footer (actions) above/below the surface. `align="end"` writes data-align
 * and reverses the row — no per-side classes needed.
 */

export function Message({
  align = "start",
  className,
  ...props
}: ComponentProps<"div"> & { readonly align?: "start" | "end" }) {
  return (
    <div
      data-slot="message"
      data-align={align}
      className={cn(
        "group/msg flex items-end gap-2.5",
        align === "end" && "flex-row-reverse",
        className,
      )}
      {...props}
    />
  );
}

/** Avatar slot — anchored to the bottom of the row, keeps its column width. */
export function MessageAvatar({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="message-avatar"
      className={cn("flex w-7 shrink-0 justify-center", className)}
      {...props}
    />
  );
}

/** The content column — holds the surface plus header/footer. */
export function MessageContent({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="message-content"
      className={cn(
        "flex min-w-0 flex-1 flex-col gap-1.5",
        // Pull surfaces to the row's side.
        "group-data-[align=end]/msg:items-end",
        className,
      )}
      {...props}
    />
  );
}

/** Small muted line above the surface — time, sender. */
export function MessageHeader({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="message-header"
      className={cn("flex items-center gap-2 px-1 text-xs text-muted-foreground", className)}
      {...props}
    />
  );
}

/** Under the surface — actions + metadata, revealed on hover/focus. */
export function MessageFooter({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="message-footer"
      className={cn(
        "flex items-center gap-1 px-1 text-xs text-muted-foreground",
        // Hover-reveal on pointer devices; always visible on touch.
        "md:opacity-0 md:transition-opacity md:group-focus-within/msg:opacity-100 md:group-hover/msg:opacity-100",
        className,
      )}
      {...props}
    />
  );
}

/** Copy action — copies raw message text, flips to a check briefly. */
export function MessageCopy({ text }: { readonly text: () => string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      variant="ghost"
      size="icon-xs"
      aria-label={copied ? "Copied" : "Copy message"}
      title={copied ? "Copied" : "Copy"}
      onClick={() => {
        void navigator.clipboard.writeText(text()).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        });
      }}
    >
      <HugeiconsIcon icon={copied ? CheckIcon : Copy01Icon} strokeWidth={2} />
    </Button>
  );
}
