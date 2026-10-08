"use client";

import { cjk } from "@streamdown/cjk";
import { math } from "@streamdown/math";
import { cn } from "cn";
import { memo, type ComponentProps } from "react";
import { Streamdown } from "streamdown";
import { StreamdownCodeBlock } from "./code-block";
// shiki and mermaid stay out of the initial bundle — see streamdown-plugins.ts
import { lazyCode, lazyMermaid } from "./streamdown-plugins";
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

const streamdownPlugins = { cjk, code: lazyCode, math, mermaid: lazyMermaid };

export const MessageResponse = memo(
  ({ className, ...props }: MessageResponseProps) => (
    <Streamdown
      className={cn(
        // wrap-anywhere keeps unbreakable tokens (long paths, identifiers,
        // inline code) inside the column instead of clipping off-viewport.
        "size-full wrap-anywhere [&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
        // Tailwind typography — consistent rhythm/spacing for p/ul/ol/h*/
        // blockquote/table/code across user + assistant bubbles.
        "prose prose-sm prose-invert max-w-none",
        className,
      )}
      plugins={streamdownPlugins}
      // Dark-only app — pin both theme slots to github-dark so highlighted
      // tokens never paint light-theme colors.
      shikiTheme={["github-dark", "github-dark"]}
      // StreamdownCodeBlock supplies its own header + copy button — drop
      // streamdown's floating code controls (table/mermaid keep theirs) and
      // the line-number gutter, which is noise inside a chat bubble.
      controls={{ code: false }}
      lineNumbers={false}
      components={{ pre: StreamdownCodeBlock, table: StreamdownTable }}
      {...props}
    />
  ),
  (prevProps, nextProps) =>
    prevProps.children === nextProps.children && nextProps.isAnimating === prevProps.isAnimating,
);

MessageResponse.displayName = "MessageResponse";
