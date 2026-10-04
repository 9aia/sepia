import { ArrowUp01Icon, Cancel01Icon, StopIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { ChatStatus, FileUIPart } from "ai";
import type {
  ComponentProps,
  FormEvent,
  FormEventHandler,
  HTMLAttributes,
  KeyboardEvent,
} from "react";
import { cn } from "cn";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupTextarea,
} from "./ui/input-group";
import { Spinner } from "./ui/spinner";

export interface PromptInputMessage {
  text: string;
  files: FileUIPart[];
}

export type PromptInputProps = Omit<HTMLAttributes<HTMLFormElement>, "onSubmit"> & {
  onSubmit: (message: PromptInputMessage, event: FormEvent<HTMLFormElement>) => void;
};

export function PromptInput({ className, onSubmit, children, ...props }: PromptInputProps) {
  const handleSubmit: FormEventHandler<HTMLFormElement> = (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const entry = new FormData(form).get("message");
    const text = typeof entry === "string" ? entry : "";
    // Reset immediately after capturing — new keystrokes land on a clean box.
    form.reset();
    onSubmit({ text, files: [] }, event);
  };

  return (
    <form className={cn("w-full", className)} onSubmit={handleSubmit} {...props}>
      <InputGroup className="overflow-hidden">{children}</InputGroup>
    </form>
  );
}

export function PromptInputBody({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("contents", className)} {...props} />;
}

export function PromptInputTextarea({
  onKeyDown,
  className,
  ...props
}: ComponentProps<typeof InputGroupTextarea>) {
  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented) return;
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      const { form } = event.currentTarget;
      const submitButton = form?.querySelector<HTMLButtonElement>('button[type="submit"]');
      if (submitButton?.disabled) return;
      form?.requestSubmit();
    }
  };

  return (
    <InputGroupTextarea
      className={cn("field-sizing-content max-h-48 min-h-16", className)}
      name="message"
      onKeyDown={handleKeyDown}
      {...props}
    />
  );
}

export function PromptInputFooter({
  className,
  ...props
}: Omit<ComponentProps<typeof InputGroupAddon>, "align">) {
  return (
    <InputGroupAddon
      align="block-end"
      className={cn("justify-between gap-1", className)}
      {...props}
    />
  );
}

export function PromptInputTools({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("flex min-w-0 items-center gap-1", className)} {...props} />;
}

export type PromptInputSubmitProps = ComponentProps<typeof InputGroupButton> & {
  status?: ChatStatus;
  onStop?: () => void;
};

export function PromptInputSubmit({
  status,
  onStop,
  onClick,
  variant = "ghost",
  size = "icon-sm",
  children,
  ...props
}: PromptInputSubmitProps) {
  const isGenerating = status === "submitted" || status === "streaming";

  const icon =
    status === "submitted" ? (
      <Spinner />
    ) : status === "streaming" ? (
      <HugeiconsIcon icon={StopIcon} strokeWidth={2} />
    ) : status === "error" ? (
      <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
    ) : (
      <HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2} />
    );

  const handleClick = (
    event: Parameters<NonNullable<ComponentProps<typeof InputGroupButton>["onClick"]>>[0],
  ) => {
    if (isGenerating && onStop !== undefined) {
      event.preventDefault();
      onStop();
      return;
    }
    onClick?.(event);
  };

  return (
    <InputGroupButton
      aria-label={isGenerating ? "Stop" : "Send"}
      onClick={handleClick}
      size={size}
      type={isGenerating && onStop !== undefined ? "button" : "submit"}
      variant={variant}
      {...props}
    >
      {children ?? icon}
    </InputGroupButton>
  );
}
