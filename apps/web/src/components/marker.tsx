import type { ComponentProps, ReactNode } from "react";
import { cn } from "@/lib/utils";

interface MarkerProps extends ComponentProps<"div"> {
  readonly variant?: "default" | "separator" | "border";
  readonly children?: ReactNode;
}

/**
 * One line of secondary status text with an optional glyph — tool-call rows,
 * run lifecycle lines, labeled dividers. The chrome between messages, not a
 * message. MarkerIcon is aria-hidden so the glyph stays decorative; status
 * belongs on the root (role="status") or a Badge inside the content.
 */
function Marker({ variant = "default", className, children, ...props }: MarkerProps) {
  return (
    <div
      data-slot="marker"
      className={cn(
        "flex items-center gap-2 px-4 py-1.5 text-xs text-muted-foreground",
        variant === "border" && "border-b border-border",
        className,
      )}
      {...props}
    >
      {variant === "separator" && <span aria-hidden className="h-px min-w-4 flex-1 bg-border" />}
      {children}
      {variant === "separator" && <span aria-hidden className="h-px min-w-4 flex-1 bg-border" />}
    </div>
  );
}

function MarkerIcon({ className, ...props }: ComponentProps<"span">) {
  return (
    <span
      aria-hidden
      data-slot="marker-icon"
      className={cn("flex size-4 shrink-0 items-center justify-center [&_svg]:size-3.5", className)}
      {...props}
    />
  );
}

function MarkerContent({ className, ...props }: ComponentProps<"div">) {
  return (
    <div data-slot="marker-content" className={cn("min-w-0 break-words", className)} {...props} />
  );
}

export { Marker, MarkerIcon, MarkerContent };
