import { useState, type FormEvent, type RefObject } from "react";
import type { AgentInfo } from "../../lib/types";
import { useCreateSession } from "../../hooks/query/useCreateSession";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../ui/select";

interface CreateFormProps {
  readonly cwdRef: RefObject<HTMLInputElement | null>;
  readonly agents: ReadonlyArray<AgentInfo>;
}

export function CreateForm({ cwdRef, agents }: CreateFormProps) {
  const createMutation = useCreateSession();
  const [cwd, setCwd] = useState("");
  const [title, setTitle] = useState("");
  const [agent, setAgent] = useState("devin");

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    const trimmedCwd = cwd.trim();
    if (trimmedCwd === "") return;
    const trimmedTitle = title.trim();
    createMutation.mutate({
      cwd: trimmedCwd,
      agent,
      title: trimmedTitle === "" ? undefined : trimmedTitle,
    });
    setCwd("");
    setTitle("");
  };

  return (
    <form className="flex flex-col gap-1.5 border-b border-border px-4 py-3" onSubmit={submit}>
      <Input
        type="text"
        placeholder="Working directory (absolute)"
        aria-label="Working directory"
        ref={cwdRef}
        value={cwd}
        onChange={(event) => setCwd(event.target.value)}
      />
      <Input
        type="text"
        placeholder="Title (optional)"
        aria-label="Session title"
        value={title}
        onChange={(event) => setTitle(event.target.value)}
      />
      {agents.length > 1 && (
        <Select
          value={agent}
          onValueChange={(value) => {
            if (value !== null) setAgent(value);
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
      <Button type="submit" disabled={createMutation.isPending || cwd.trim() === ""}>
        {createMutation.isPending ? "Creating…" : "New session"}
      </Button>
      {createMutation.isError && (
        <p className="p-4 text-destructive">
          {createMutation.error instanceof Error
            ? createMutation.error.message
            : "Failed to create session"}
        </p>
      )}
    </form>
  );
}
