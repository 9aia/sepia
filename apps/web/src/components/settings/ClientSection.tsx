import { useState, type KeyboardEvent } from "react";
import { Copy01Icon, RefreshIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { regenerateClient, setClientLabel, truncateKey, useClient } from "../../lib/client";
import { SECRET_MASK } from "../../lib/servers";
import { toastError, toastSuccess } from "../../lib/toast";
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

/** Copy helper — clipboard APIs can be absent on insecure origins. */
const copy = (value: string): void => {
  void navigator.clipboard?.writeText(value).then(
    () => toastSuccess("Copied to clipboard"),
    () => undefined,
  );
};

/**
 * Settings → Client: this client's identity — the label + keypair other
 * clients will know it by once pairing/sync exists. The public key is
 * shareable (truncated display + copy); the secret stays masked and local;
 * Regenerate re-keys under the same id, behind a confirm since it changes
 * the identity peers would recognize.
 */
export function ClientSection() {
  const client = useClient();
  const [confirming, setConfirming] = useState(false);

  return (
    <section data-spy="client" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Client</h3>
      <p className="text-xs text-muted-foreground">
        This client&apos;s identity — the label and keypair that name this device. The public key is
        shareable; the secret never leaves this client.
      </p>
      {client === null ? (
        <p className="text-xs text-muted-foreground">Generating keys…</p>
      ) : (
        <div className="divide-y divide-border/50 rounded-lg border border-border">
          <div className="flex items-center gap-3 px-3 py-2.5">
            <div className="min-w-0 flex-1">
              <span className="block text-xs text-muted-foreground">Label</span>
              <Input
                // Re-seed the field when the stored label changes elsewhere.
                key={client.label}
                className="h-7 w-44 max-w-full text-sm font-medium"
                defaultValue={client.label}
                placeholder="Client label"
                aria-label="Client label"
                onBlur={(event) => setClientLabel(event.currentTarget.value)}
                onKeyDown={blurOnEnter}
              />
            </div>
          </div>
          <div className="flex items-center gap-3 px-3 py-2.5">
            <div className="min-w-0 flex-1">
              <span className="block text-xs text-muted-foreground">Client ID</span>
              <span className="block truncate font-mono text-xs" title={client.id}>
                {client.id}
              </span>
            </div>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Copy client ID"
              title="Copy client ID"
              onClick={() => copy(client.id)}
            >
              <HugeiconsIcon icon={Copy01Icon} strokeWidth={2} />
            </Button>
          </div>
          <div className="flex items-center gap-3 px-3 py-2.5">
            <div className="min-w-0 flex-1">
              <span className="block text-xs text-muted-foreground">
                Public key{client.algorithm !== "none" ? ` · ${client.algorithm}` : ""}
              </span>
              <span className="block truncate font-mono text-xs" title={client.publicKey}>
                {client.publicKey === ""
                  ? "Unavailable — no usable crypto in this context or node"
                  : truncateKey(client.publicKey)}
              </span>
            </div>
            {client.publicKey !== "" && (
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Copy public key"
                title="Copy public key"
                onClick={() => copy(client.publicKey)}
              >
                <HugeiconsIcon icon={Copy01Icon} strokeWidth={2} />
              </Button>
            )}
          </div>
          <div className="flex items-center gap-3 px-3 py-2.5">
            <div className="min-w-0 flex-1">
              <span className="block text-xs text-muted-foreground">Secret key</span>
              <span className="block truncate font-mono text-xs" title="Stored locally, masked">
                {SECRET_MASK}
              </span>
            </div>
          </div>
        </div>
      )}
      {client !== null && (
        <div className="flex justify-end">
          <Button variant="outline" size="xs" onClick={() => setConfirming(true)}>
            <HugeiconsIcon icon={RefreshIcon} strokeWidth={2} />
            Regenerate keys…
          </Button>
        </div>
      )}
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Regenerate client keys?</AlertDialogTitle>
            <AlertDialogDescription>
              This rotates the client&apos;s keypair — anything that recognizes this client by its
              public key will need to verify it again. The label and client id stay the same.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                void regenerateClient()
                  .then(() => toastSuccess("Client keys regenerated"))
                  .catch(() => toastError("Couldn't regenerate the client keys"));
                setConfirming(false);
              }}
            >
              Regenerate
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
