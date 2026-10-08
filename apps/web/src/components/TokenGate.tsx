import { useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { LockKeyholeIcon } from "@hugeicons/core-free-icons";
import { login, AuthError } from "../lib/api";
import { setAuthBlocked } from "../lib/store";
import { EmptyScreen } from "./EmptyScreen";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

export function TokenGate() {
  const queryClient = useQueryClient();
  const [value, setValue] = useState("");

  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent): Promise<void> => {
    event.preventDefault();
    const token = value.trim();
    if (token === "") return;
    setError(null);
    try {
      await login(token);
      setAuthBlocked(false);
      void queryClient.invalidateQueries();
    } catch (err) {
      setError(err instanceof AuthError ? "Invalid token" : "Login failed — try again");
    }
  };

  return (
    <main className="flex min-h-svh bg-background">
      <EmptyScreen
        className="m-auto max-w-xl"
        icon={LockKeyholeIcon}
        title="Authentication required"
        description="This Sepia node is protected. Enter your SEPIA_TOKEN to continue."
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
          {error === null ? null : <p className="text-center text-xs text-destructive">{error}</p>}
        </form>
      </EmptyScreen>
    </main>
  );
}
