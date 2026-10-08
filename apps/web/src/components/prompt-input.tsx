import {
  SentIcon,
  AttachmentIcon,
  Cancel01Icon,
  File01Icon,
  StopIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type ClipboardEvent,
  type ComponentProps,
  type DragEvent,
  type FormEvent,
  type FormEventHandler,
  type HTMLAttributes,
  type KeyboardEvent,
  type RefObject,
} from "react";
import { cn } from "cn";
import {
  attachmentBudgetError,
  fileToAttachment,
  formatAttachmentSize,
  type PendingAttachment,
} from "../lib/attachments";
import { toastError } from "../lib/toast";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupTextarea,
} from "./ui/input-group";
import { Spinner } from "./ui/spinner";

/** Same shape as the AI SDK's `ChatStatus`, inlined to avoid depending on `ai`. */
export type ChatStatus = "submitted" | "streaming" | "ready" | "error";

export interface PromptInputMessage {
  text: string;
  attachments: PendingAttachment[];
}

interface PromptInputContextValue {
  readonly attachments: ReadonlyArray<PendingAttachment>;
  readonly addFiles: (files: Iterable<File>) => void;
  readonly removeAttachment: (id: string) => void;
  /** Empties the tray — for a deferred send that kept its draft on submit. */
  readonly clearAttachments: () => void;
}

/** Present only inside `<PromptInput>` — children opt into the shared attachment tray. */
const PromptInputContext = createContext<PromptInputContextValue | null>(null);

export type PromptInputProps = Omit<HTMLAttributes<HTMLFormElement>, "onSubmit"> & {
  /**
   * Returning `false` keeps the draft in the composer (text + chips) — a
   * queued send, like the held-session takeover path, flushes the box itself
   * once the message actually lands.
   */
  onSubmit: (message: PromptInputMessage, event: FormEvent<HTMLFormElement>) => false | void;
};

export function PromptInput({ className, onSubmit, children, ...props }: PromptInputProps) {
  const [attachments, setAttachments] = useState<PendingAttachment[]>([]);
  const [dragging, setDragging] = useState(false);
  // Files being read count against the budget before they exist as
  // attachments — otherwise two quick drops can race past the cap.
  const reserved = useRef({ count: 0, bytes: 0 });

  const addFiles = (files: Iterable<File>): void => {
    const list = [...files];
    if (list.length === 0) return;
    const error = attachmentBudgetError(attachments, list, reserved.current);
    if (error !== null) {
      toastError(error);
      return;
    }
    reserved.current = {
      count: reserved.current.count + list.length,
      bytes: reserved.current.bytes + list.reduce((sum, file) => sum + file.size, 0),
    };
    const release = (): void => {
      reserved.current = {
        count: reserved.current.count - list.length,
        bytes: reserved.current.bytes - list.reduce((sum, file) => sum + file.size, 0),
      };
    };
    void Promise.all(list.map((file) => fileToAttachment(file)))
      .then((loaded) => {
        release();
        setAttachments((prev) => [...prev, ...loaded]);
      })
      .catch((readError: unknown) => {
        release();
        toastError("Couldn't read a file", readError);
      });
  };

  const removeAttachment = (id: string): void => {
    setAttachments((prev) => prev.filter((attachment) => attachment.id !== id));
  };

  const clearAttachments = (): void => {
    setAttachments([]);
  };

  const handleSubmit: FormEventHandler<HTMLFormElement> = (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const entry = new FormData(form).get("message");
    const text = typeof entry === "string" ? entry : "";
    const sent = attachments;
    // `false` keeps the draft — the caller queued it behind something (a
    // takeover confirm, a busy turn) and owns flushing the box later.
    if (onSubmit({ text, attachments: sent }, event) === false) return;
    // Reset immediately after capturing — new keystrokes land on a clean box.
    form.reset();
    setAttachments([]);
  };

  const onDragOver = (event: DragEvent<HTMLFormElement>): void => {
    if (!event.dataTransfer.types.includes("Files")) return;
    event.preventDefault();
    setDragging(true);
  };
  const onDragLeave = (event: DragEvent<HTMLFormElement>): void => {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
    setDragging(false);
  };
  const onDrop = (event: DragEvent<HTMLFormElement>): void => {
    if (event.dataTransfer.files.length === 0) return;
    event.preventDefault();
    setDragging(false);
    addFiles(event.dataTransfer.files);
  };

  return (
    <PromptInputContext.Provider
      value={{ attachments, addFiles, removeAttachment, clearAttachments }}
    >
      <form
        className={cn("w-full", className)}
        onSubmit={handleSubmit}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onDrop={onDrop}
        {...props}
      >
        <InputGroup
          className={cn("overflow-hidden", dragging && "border-ring ring-3 ring-ring/30")}
        >
          {children}
        </InputGroup>
      </form>
    </PromptInputContext.Provider>
  );
}

export function PromptInputBody({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("contents", className)} {...props} />;
}

/**
 * `useContext(PromptInputContext)` only works under the provider — this
 * bridge hands `clearAttachments` out through a ref so a queued draft (kept
 * in the composer on submit) can be emptied when its deferred send lands.
 */
export function PromptInputApiBridge({
  apiRef,
}: {
  readonly apiRef: RefObject<(() => void) | null>;
}) {
  const context = useContext(PromptInputContext);
  useEffect(() => {
    apiRef.current = context?.clearAttachments ?? null;
    return () => {
      apiRef.current = null;
    };
  }, [apiRef, context]);
  return null;
}

export function PromptInputTextarea({
  onKeyDown,
  onPaste,
  className,
  ...props
}: ComponentProps<typeof InputGroupTextarea>) {
  const context = useContext(PromptInputContext);

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

  const handlePaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    onPaste?.(event);
    if (event.defaultPrevented) return;
    // Only intercept file payloads — a plain text paste stays a text paste.
    if (event.clipboardData.files.length === 0) return;
    event.preventDefault();
    context?.addFiles(event.clipboardData.files);
  };

  return (
    <InputGroupTextarea
      className={cn("field-sizing-content max-h-48 min-h-16", className)}
      name="message"
      onKeyDown={handleKeyDown}
      onPaste={handlePaste}
      {...props}
    />
  );
}

/** Chip row above the textarea — renders nothing until files land. */
export function PromptInputAttachments({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  const context = useContext(PromptInputContext);
  if (context === null || context.attachments.length === 0) return null;
  return (
    <div className={cn("flex flex-wrap items-center gap-1.5 px-3 pt-2.5", className)} {...props}>
      {context.attachments.map((attachment) => (
        <span
          key={attachment.id}
          title={`${attachment.name} · ${formatAttachmentSize(attachment.size)}`}
          className="inline-flex max-w-56 items-center gap-2 rounded-lg border border-border/60 bg-muted/40 py-1.5 pr-1.5 pl-1.5 text-xs"
        >
          {attachment.previewUrl !== undefined ? (
            <img
              src={attachment.previewUrl}
              alt={attachment.name}
              className="size-7 shrink-0 rounded-md object-cover"
            />
          ) : (
            <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-background text-muted-foreground">
              <HugeiconsIcon icon={File01Icon} className="size-4" strokeWidth={2} />
            </span>
          )}
          <span className="flex min-w-0 flex-col leading-tight">
            <span className="truncate font-medium text-foreground/90">{attachment.name}</span>
            <span className="truncate text-muted-foreground/70">
              {formatAttachmentSize(attachment.size)}
            </span>
          </span>
          <button
            type="button"
            aria-label={`Remove ${attachment.name}`}
            className="shrink-0 rounded p-0.5 text-muted-foreground/70 hover:bg-muted hover:text-foreground"
            onClick={() => context.removeAttachment(attachment.id)}
          >
            <HugeiconsIcon icon={Cancel01Icon} className="size-3.5" strokeWidth={2} />
          </button>
        </span>
      ))}
    </div>
  );
}

/** Paperclip footer tool — opens a file picker; any file type, images + text are the priority. */
export function PromptInputAttachButton({
  onClick,
  children,
  ...props
}: ComponentProps<typeof InputGroupButton>) {
  const context = useContext(PromptInputContext);
  const inputRef = useRef<HTMLInputElement>(null);

  const onChange = (event: ChangeEvent<HTMLInputElement>): void => {
    if (event.target.files !== null) context?.addFiles(event.target.files);
    // Same file re-picked must fire onChange again.
    event.target.value = "";
  };

  return (
    <>
      <input ref={inputRef} type="file" multiple hidden onChange={onChange} />
      <InputGroupButton
        aria-label="Attach files"
        title="Attach files"
        disabled={context === null}
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) inputRef.current?.click();
        }}
        {...props}
      >
        {children ?? <HugeiconsIcon icon={AttachmentIcon} strokeWidth={2} />}
      </InputGroupButton>
    </>
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
      <HugeiconsIcon icon={SentIcon} strokeWidth={2} />
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
