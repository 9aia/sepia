"use client";

import { cjk } from "@streamdown/cjk";
import { code } from "@streamdown/code";
import { math } from "@streamdown/math";
import { mermaid } from "@streamdown/mermaid";
import { cn } from "cn";
import { memo, type ComponentProps } from "react";
import { Streamdown } from "streamdown";
import { StreamdownCodeBlock } from "./code-block";
import { ScrollArea } from "./ui/scroll-area";

export type MessageResponseProps = ComponentProps<typeof Streamdown>;

/** Data tables can't wrap — give them their own horizontal scroll track
 *  instead of widening the whole message column. */
function StreamdownTable({
  children,
  node: _node,
  ...props
}: ComponentProps<"table"> & { readonly node?: unknown }) {
  return (
    <ScrollArea>
      <table {...props}>{children}</table>
    </ScrollArea>
  );
}

const streamdownPlugins = { cjk, code, math, mermaid };

export const MessageResponse = memo(
  ({ className, ...props }: MessageResponseProps) => (
    <Streamdown
      className={cn(
        // wrap-anywhere keeps unbreakable tokens (long paths, identifiers,
        // inline code) inside the column instead of clipping off-viewport.
        "size-full wrap-anywhere [&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
        className,
      )}
      plugins={streamdownPlugins}
      components={{ pre: StreamdownCodeBlock, table: StreamdownTable }}
      {...props}
    />
  ),
  (prevProps, nextProps) =>
    prevProps.children === nextProps.children && nextProps.isAnimating === prevProps.isAnimating,
);

MessageResponse.displayName = "MessageResponse";
