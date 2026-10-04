import type { ComponentProps, ReactNode } from "react";
import { cn } from "@/lib/utils";

type BubbleVariant =
  | "default"
  | "secondary"
  | "muted"
  | "tinted"
  | "outline"
  | "ghost"
  | "destructive";

interface BubbleProps extends ComponentProps<"div"> {
  readonly align?: "start" | "end";
  readonly variant?: BubbleVariant;
  readonly children?: ReactNode;
}

const variantClass: Record<BubbleVariant, string> = {
  default: "bg-secondary text-foreground",
  secondary: "bg-secondary/60 text-foreground",
  muted: "bg-muted text-foreground",
  tinted: "bg-primary/15 text-foreground",
  outline: "border border-border bg-transparent text-foreground",
  ghost: "bg-transparent text-foreground",
  destructive: "bg-destructive/10 text-destructive",
};

/**
 * Consecutive turns from one sender — the tighter internal gap is the whole
 * grouping signal, so don't equalize it with the thread gap.
 */
function BubbleGroup({ className, ...props }: ComponentProps<"div">) {
  return (
    <div data-slot="bubble-group" className={cn("flex flex-col gap-1", className)} {...props} />
  );
}

/**
 * One message's container — the side comes from `align`, the color from
 * `variant`. Sizes to content, capped at 80% of the thread. `ghost` clears
 * the frame and the cap — that's the variant assistant prose wants so it
 * reads as a document, not a quoted chat turn.
 */
function Bubble({ align = "start", variant = "default", className, ...props }: BubbleProps) {
  return (
    <div
      data-slot="bubble"
      className={cn(
        "w-fit max-w-[80%] min-w-0 flex-col gap-2 self-start rounded-2xl text-sm",
        variant === "ghost" && "max-w-full self-stretch",
        align === "end" && "ml-auto",
        className,
      )}
      {...props}
    />
  );
}

function BubbleContent({
  variant = "default",
  className,
  ...props
}: ComponentProps<"div"> & { readonly variant?: BubbleVariant }) {
  return (
    <div
      data-slot="bubble-content"
      className={cn(
        "w-fit max-w-full min-w-0 overflow-hidden rounded-2xl px-4 py-3",
        variantClass[variant],
        variant === "ghost" && "max-w-none overflow-visible rounded-none px-0 py-0",
        className,
      )}
      {...props}
    />
  );
}

export { Bubble, BubbleContent, BubbleGroup };
export type { BubbleVariant };
