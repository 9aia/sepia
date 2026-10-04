import { useEffect, type FormEvent, type RefObject } from "react";
import { useForm } from "@tanstack/react-form";
import { ArrowDown01Icon, ArrowUp01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { AgentInfo } from "../../lib/types";
import { useCreateSession } from "../../hooks/query/useCreateSession";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../ui/select";

interface CreateFormProps {
  readonly cwdRef: RefObject<HTMLInputElement | null>;
  readonly agents: ReadonlyArray<AgentInfo>;
  /** Pre-fills cwd — the most recently updated session's directory. */
  readonly defaultCwd: string | undefined;
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function CreateForm({ cwdRef, agents, defaultCwd, open, onOpenChange }: CreateFormProps) {
  const createMutation = useCreateSession();
  const form = useForm({
    defaultValues: {
      cwd: defaultCwd ?? "",
      title: "",
      agent: agents[0]?.id ?? "devin",
    },
    onSubmit: async ({ value }) => {
      try {
        await createMutation.mutateAsync({
          cwd: value.cwd.trim(),
          agent: value.agent,
          title: value.title.trim() === "" ? undefined : value.title.trim(),
        });
        form.reset();
        onOpenChange(false);
      } catch {
        // isError renders below.
      }
    },
  });

  // Late-arriving defaults (sessions/agents resolve after mount) fill fields
  // the user hasn't touched.
  useEffect(() => {
    if (defaultCwd !== undefined && form.getFieldValue("cwd") === "") {
      form.setFieldValue("cwd", defaultCwd);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultCwd]);
  useEffect(() => {
    const first = agents[0]?.id;
    if (first !== undefined && form.getFieldValue("agent") === "") {
      form.setFieldValue("agent", first);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agents]);

  // One-click create with the prefilled defaults; opens the advanced section
  // instead when there's nothing to prefill.
  const quickCreate = (): void => {
    const cwd = form.getFieldValue("cwd").trim();
    if (cwd === "") {
      onOpenChange(true);
      requestAnimationFrame(() => cwdRef.current?.focus());
      return;
    }
    const title = form.getFieldValue("title").trim();
    createMutation.mutate({
      cwd,
      agent: form.getFieldValue("agent"),
      title: title === "" ? undefined : title,
    });
  };

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    void form.handleSubmit();
  };

  return (
    <div className="flex flex-col gap-1.5 border-b border-border px-4 py-3">
      <div className="flex gap-1.5">
        <Button
          type="button"
          className="flex-1"
          onClick={quickCreate}
          disabled={createMutation.isPending}
        >
          {createMutation.isPending ? "Creating…" : "New session"}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="icon"
          aria-label={open ? "Hide advanced options" : "Show advanced options"}
          aria-expanded={open}
          onClick={() => onOpenChange(!open)}
        >
          <HugeiconsIcon icon={open ? ArrowUp01Icon : ArrowDown01Icon} strokeWidth={2} />
        </Button>
      </div>

      {open && (
        <form className="flex flex-col gap-1.5" onSubmit={submit}>
          <form.Field
            name="cwd"
            validators={{
              onChange: ({ value }) => {
                const trimmed = value.trim();
                if (trimmed === "") return "Working directory is required";
                if (!/^[~/]|^[a-zA-Z]:[\\/]/.test(trimmed)) return "Must be an absolute path";
                return undefined;
              },
            }}
          >
            {(field) => (
              <>
                <Input
                  type="text"
                  placeholder="Working directory (absolute)"
                  aria-label="Working directory"
                  aria-invalid={field.state.meta.errors.length > 0}
                  ref={cwdRef}
                  value={field.state.value}
                  onBlur={field.handleBlur}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
                {field.state.meta.isTouched && field.state.meta.errors[0] !== undefined && (
                  <p className="text-xs text-destructive">{field.state.meta.errors[0]}</p>
                )}
              </>
            )}
          </form.Field>
          <form.Field name="title">
            {(field) => (
              <Input
                type="text"
                placeholder="Title (optional)"
                aria-label="Session title"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
              />
            )}
          </form.Field>
          {agents.length > 1 && (
            <form.Field name="agent">
              {(field) => (
                <Select
                  value={field.state.value}
                  onValueChange={(value) => {
                    if (value !== null) field.handleChange(value);
                  }}
                >
                  <SelectTrigger aria-label="Agent">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {agents.map((a) => (
                      <SelectItem key={a.id} value={a.id}>
                        {a.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            </form.Field>
          )}
          <Button type="submit" disabled={createMutation.isPending}>
            {createMutation.isPending ? "Creating…" : "Create with options"}
          </Button>
        </form>
      )}

      {createMutation.isError && (
        <p className="text-xs text-destructive">
          {createMutation.error instanceof Error
            ? createMutation.error.message
            : "Failed to create session"}
        </p>
      )}
    </div>
  );
}
