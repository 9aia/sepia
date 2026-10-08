import {
  cloneElement,
  isValidElement,
  useRef,
  useState,
  type ComponentProps,
  type JSX,
  type ReactNode,
} from "react";
import { CheckIcon, Copy01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { cn } from "@/lib/utils";
import { Button } from "./ui/button";

/**
 * The chrome around a rendered <pre>: a header strip with the language and a
 * copy button. Children stay untouched — streamdown/Shiki's highlighted
 * markup renders inside, so this adds chrome, not a second highlighter.
 */

/** Pull `language-xyz` off a fence's <code> element (streamdown's className). */
const codeLanguage = (node: ReactNode): string | null => {
  if (!isValidElement<{ className?: string }>(node)) return null;
  const cls = node.props.className ?? "";
  const match = cls.match(/language-(\S+)/);
  return match?.[1] ?? null;
};

const rawText = (el: HTMLElement | null): string =>
  el?.querySelector("code")?.textContent ?? el?.textContent ?? "";

function CodeBlockCopy({ target }: { readonly target: () => HTMLElement | null }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      variant="ghost"
      size="icon-xs"
      aria-label={copied ? "Copied" : "Copy code"}
      title={copied ? "Copied" : "Copy"}
      onClick={() => {
        const text = rawText(target());
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        });
      }}
    >
      <HugeiconsIcon icon={copied ? CheckIcon : Copy01Icon} strokeWidth={2} />
    </Button>
  );
}

interface CodeBlockProps extends ComponentProps<"div"> {
  readonly language?: string | null;
  readonly children?: ReactNode;
}

function CodeBlock({ language, className, children, ...props }: CodeBlockProps) {
  const preRef = useRef<HTMLDivElement>(null);
  return (
    <div
      data-slot="code-block"
      className={cn(
        "group/code overflow-hidden rounded-xl border border-border bg-muted/40",
        "not-prose",
        className,
      )}
      {...props}
    >
      <div className="flex items-center justify-between border-b border-border bg-muted/60 px-3 py-1">
        <span className="text-[11px] tracking-wide text-muted-foreground uppercase">
          {language ?? "code"}
        </span>
        <CodeBlockCopy target={() => preRef.current} />
      </div>
      <div
        ref={preRef}
        // children is streamdown's own fenced-block render (container +
        // language header + highlighted body). Our chrome replaces that
        // chrome, so strip streamdown's container/header/body styling and
        // keep the highlighted <pre> (its --sdm-bg theme background stays).
        className={cn(
          "overflow-x-auto p-3 [&_pre]:!m-0 [&_pre]:!rounded-none [&_pre]:!border-0 [&_pre]:!p-0",
          "[&_[data-streamdown=code-block]]:!my-0 [&_[data-streamdown=code-block]]:!gap-0",
          "[&_[data-streamdown=code-block]]:!rounded-none [&_[data-streamdown=code-block]]:!border-0",
          "[&_[data-streamdown=code-block]]:!bg-transparent [&_[data-streamdown=code-block]]:!p-0",
          "[&_[data-streamdown=code-block-header]]:!hidden",
          "[&_[data-streamdown=code-block-body]]:!rounded-none [&_[data-streamdown=code-block-body]]:!border-0",
          "[&_[data-streamdown=code-block-body]]:!bg-transparent [&_[data-streamdown=code-block-body]]:!p-0",
        )}
      >
        {children}
      </div>
    </div>
  );
}

/**
 * streamdown `pre` renderer. streamdown's default `pre` never emits a <pre> —
 * it flags its <code> child with `data-block` so the `code` renderer takes
 * the fenced-block path (shiki-highlighted body, mermaid diagram, custom
 * renderer) instead of inline code. Forwarding children without that flag
 * silently downgraded every fence to unhighlighted inline code — and the
 * language sits on the code element itself, not on its children.
 */
export function StreamdownCodeBlock({
  children,
}: JSX.IntrinsicElements["pre"] & { readonly node?: unknown }) {
  const block = isValidElement<Record<string, unknown>>(children)
    ? cloneElement(children, { "data-block": "true" })
    : children;
  const language = codeLanguage(children);
  // A mermaid fence renders the diagram itself — no code chrome around it.
  if (language === "mermaid") return block;
  return <CodeBlock language={language}>{block}</CodeBlock>;
}

export { CodeBlock, CodeBlockCopy };
