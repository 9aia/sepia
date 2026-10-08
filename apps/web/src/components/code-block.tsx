import {
  useRef,
  useState,
  type ComponentProps,
  type JSX,
  type ReactElement,
  type ReactNode,
} from "react";
import { isValidElement } from "react";
import { CheckIcon, Copy01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { cn } from "@/lib/utils";
import { Button } from "./ui/button";

/**
 * The chrome around a rendered <pre>: a header strip with the language and a
 * copy button. Children stay untouched — streamdown/Shiki's highlighted
 * markup renders inside, so this adds chrome, not a second highlighter.
 */

/** Pull `language-xyz` out of a <pre>'s code child, if streamdown set one. */
const codeLanguage = (node: ReactNode): string | null => {
  if (!isValidElement<{ className?: string }>(node)) return null;
  const cls = node.props.className ?? "";
  const match = cls.match(/language-(\w+)/);
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
        // children is the <code> element (streamdown passes it to the pre
        // renderer) — pad the container itself; any nested pre the code
        // plugin emits keeps chrome but no padding of its own.
        className="overflow-x-auto p-3 [&_pre]:!m-0 [&_pre]:!rounded-none [&_pre]:!border-0 [&_pre]:!p-0"
      >
        {children}
      </div>
    </div>
  );
}

/**
 * streamdown `pre` renderer — reads the language off its code child and wraps
 * the streamdown/Shiki output in the CodeBlock chrome.
 */
export function StreamdownCodeBlock({
  children,
}: JSX.IntrinsicElements["pre"] & { readonly node?: unknown }) {
  const child = children as ReactElement<{ children?: ReactNode }> | undefined;
  const codeChild = child?.props?.children;
  return <CodeBlock language={codeLanguage(codeChild)}>{children}</CodeBlock>;
}

export { CodeBlock, CodeBlockCopy };
