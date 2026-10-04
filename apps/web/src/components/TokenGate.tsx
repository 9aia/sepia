import { useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { LockKeyholeIcon } from "@hugeicons/core-free-icons";
import { setToken } from "../lib/api";
import { EmptyScreen } from "./EmptyScreen";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

export function TokenGate() {
  const queryClient = useQueryClient();
  const [value, setValue] = useState("");

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    const token = value.trim();
    if (token === "") return;
    setToken(token);
    void queryClient.invalidateQueries();
  };

  return (
    <main className="flex min-h-svh bg-background">
      <EmptyScreen
        className="m-auto max-w-xl"
        icon={LockKeyholeIcon}
        title="Authentication required"
        description="This Sepia server is protected. Enter your SEPIA_TOKEN to continue."
      >
        <form onSubmit={submit} className="flex w-full max-w-xs flex-col gap-2">
          <Input
            type="password"
            placeholder="API token"
            aria-label="API token"
            autoFocus
            value={value}
            onChange={(event) => setValue(event.target.value)}
          />
          <Button type="submit">Unlock</Button>
        </form>
      </EmptyScreen>
    </main>
  );
}
