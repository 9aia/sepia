import { useState, type KeyboardEvent } from "react";
import { useStore } from "@tanstack/react-store";
import { Add01Icon, Delete02Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  addCredential,
  credentialsStore,
  removeCredential,
  setCredentialLabel,
  type Credential,
} from "../../lib/credentials";
import { nodesStore } from "../../lib/nodes";
import { SECRET_MASK } from "../../lib/servers";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../ui/alert-dialog";
import { Button } from "../ui/button";
import { Input } from "../ui/input";

const blurOnEnter = (event: KeyboardEvent<HTMLInputElement>): void => {
  if (event.key === "Enter") event.currentTarget.blur();
};

/** Bottom-of-section add form — a credential is just a label + a secret. */
function CredentialAddForm() {
  const [label, setLabel] = useState("");
  const [secret, setSecret] = useState("");
  const canAdd = secret.trim() !== "";
  const submit = (): void => {
    if (!canAdd) return;
    addCredential({ label, secret: secret.trim() });
    setLabel("");
    setSecret("");
  };
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
      className="flex flex-col gap-2 rounded-lg border border-border p-3"
    >
      <div>
        <span className="block text-sm font-medium">Add a credential</span>
        <span className="block text-xs text-muted-foreground">
          A named bearer token nodes can link — secrets stay in this browser.
        </span>
      </div>
      <div className="grid gap-2 sm:grid-cols-[1fr_1fr_auto]">
        <Input
          placeholder="Label (e.g. thinkpad)"
          aria-label="Credential label"
          value={label}
          onChange={(event) => setLabel(event.target.value)}
        />
        <Input
          type="password"
          placeholder="Bearer token"
          aria-label="Credential secret"
          autoComplete="new-password"
          value={secret}
          onChange={(event) => setSecret(event.target.value)}
        />
        <Button type="submit" variant="secondary" disabled={!canAdd}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
          Add
        </Button>
      </div>
    </form>
  );
}

/**
 * Settings → Credentials: the browser-local secret store behind
 * `PeerNode.credentialId`. Each row shows the (inline-editable) label, the
 * masked secret, and the nodes linking it; remove confirms first and names
 * the referencing nodes — the link stays dangling on the peer, whose calls
 * then fail auth rather than sending a stale secret.
 */
export function CredentialsSection() {
  const credentials = useStore(credentialsStore);
  const peers = useStore(nodesStore, (state) => state.peers);
  const [removing, setRemoving] = useState<Credential | null>(null);
  const usedBy = (id: string): ReadonlyArray<string> =>
    peers.filter((peer) => peer.credentialId === id).map((peer) => peer.alias ?? peer.name);
  const removingRefs = removing === null ? [] : usedBy(removing.id);

  return (
    <section data-spy="credentials" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Credentials</h3>
      <p className="text-xs text-muted-foreground">
        Bearer tokens this browser holds for direct nodes — pick them when adding or editing a node
        instead of re-typing secrets. Gateway-routed nodes keep theirs in the managed server
        registry instead.
      </p>
      {credentials.length > 0 && (
        <div className="divide-y divide-border/50 rounded-lg border border-border">
          {credentials.map((credential) => {
            const refs = usedBy(credential.id);
            return (
              <div key={credential.id} className="flex items-center gap-3 px-3 py-2.5">
                <div className="min-w-0 flex-1">
                  <Input
                    // Re-seed the field when the stored label changes elsewhere.
                    key={credential.label}
                    className="h-7 w-44 max-w-full text-sm font-medium"
                    defaultValue={credential.label}
                    placeholder="Credential label"
                    aria-label={`Label for credential ${credential.label}`}
                    onBlur={(event) => setCredentialLabel(credential.id, event.currentTarget.value)}
                    onKeyDown={blurOnEnter}
                  />
                  <span className="block truncate text-xs text-muted-foreground">
                    {`token ${SECRET_MASK}`}
                    {refs.length === 0
                      ? " — unused"
                      : ` — used by ${refs.length === 1 ? refs[0] : `${refs.length} nodes`}`}
                  </span>
                </div>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`Remove credential ${credential.label}`}
                  title="Remove credential"
                  onClick={() => setRemoving(credential)}
                >
                  <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                </Button>
              </div>
            );
          })}
        </div>
      )}
      <CredentialAddForm />
      <AlertDialog open={removing !== null} onOpenChange={(open) => !open && setRemoving(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove credential?</AlertDialogTitle>
            <AlertDialogDescription>
              {`"${removing?.label ?? ""}" is removed from this browser.`}
              {removingRefs.length === 0
                ? ""
                : ` ${removingRefs.length} node${removingRefs.length === 1 ? "" : "s"} (${removingRefs.join(", ")}) use${removingRefs.length === 1 ? "s" : ""} this credential — its calls will fail auth until you link another.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (removing !== null) removeCredential(removing.id);
                setRemoving(null);
              }}
            >
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
