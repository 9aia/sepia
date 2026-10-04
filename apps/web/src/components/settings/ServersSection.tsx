import { useState, type FormEvent } from "react";
import { Add01Icon, Delete02Icon, PencilEdit01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  useCreateServer,
  useDeleteServer,
  useServers,
  useServerStatuses,
  useUpdateServer,
} from "../../hooks/query/useServers";
import {
  parseServerHost,
  SECRET_MASK,
  type ManagedServer,
  type ServerInput,
} from "../../lib/servers";
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
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "../ui/dialog";
import { Input } from "../ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "../ui/select";
import { Switch } from "../ui/switch";
import { Textarea } from "../ui/textarea";

/** Reachability dot — undefined while the first probe is in flight. */
function StatusDot({ ok }: { readonly ok: boolean | undefined }) {
  const color =
    ok === undefined ? "bg-muted-foreground/40" : ok ? "bg-emerald-500" : "bg-destructive";
  return (
    <span
      className={`inline-block size-2 shrink-0 rounded-full ${color}`}
      title={ok === undefined ? "Checking…" : ok ? "Online" : "Offline"}
    />
  );
}

type AuthType = "none" | "token" | "password";

interface FormState {
  label: string;
  host: string;
  port: string;
  authType: AuthType;
  authUser: string;
  authSecret: string;
  sshEnabled: boolean;
  sshHost: string;
  sshPort: string;
  sshUser: string;
  sshKey: string;
}

const EMPTY_FORM: FormState = {
  label: "",
  host: "",
  port: "8787",
  authType: "token",
  authUser: "",
  authSecret: "",
  sshEnabled: false,
  sshHost: "",
  sshPort: "22",
  sshUser: "",
  sshKey: "",
};

/** Masked secrets round-trip untouched; the server keeps the stored value. */
const formFromServer = (server: ManagedServer): FormState => ({
  label: server.label,
  // Seed the scheme into the host field — resaving a bare hostname would
  // silently parse back to http and downgrade a TLS upstream.
  host: `${server.scheme}://${server.host}`,
  port: String(server.port),
  authType: server.auth?.type ?? "none",
  authUser: server.auth?.user ?? "",
  authSecret: server.auth === null ? "" : SECRET_MASK,
  sshEnabled: server.ssh !== null,
  sshHost: server.ssh?.host ?? "",
  sshPort: String(server.ssh?.port ?? 22),
  sshUser: server.ssh?.user ?? "",
  sshKey: server.ssh?.key ?? "",
});

const formToInput = (form: FormState): ServerInput => {
  // The host field accepts `https://host[:port]` — scheme (and an explicit
  // :port) parsed out of it win over the defaults below.
  const address = parseServerHost(form.host);
  return {
    label: form.label,
    host: address?.host ?? form.host.trim(),
    port: address?.port ?? Number(form.port),
    scheme: address?.scheme ?? "http",
    auth:
      form.authType === "none"
        ? null
        : {
            type: form.authType,
            user: form.authUser.trim() === "" ? undefined : form.authUser.trim(),
            secret: form.authSecret,
          },
    ssh: form.sshEnabled
      ? {
          host: form.sshHost,
          port: Number(form.sshPort),
          user: form.sshUser,
          key: form.sshKey.trim() === "" ? undefined : form.sshKey,
        }
      : null,
  };
};

const portFieldValid = (port: string): boolean =>
  Number.isInteger(Number(port)) && Number(port) >= 1 && Number(port) <= 65535;

const formValid = (form: FormState): boolean => {
  const address = parseServerHost(form.host);
  return (
    form.label.trim() !== "" &&
    // The port field only applies when the host field didn't carry :port.
    address !== null &&
    (address.port !== null || portFieldValid(form.port)) &&
    (form.authType === "none" || form.authSecret !== "") &&
    (!form.sshEnabled || (form.sshHost.trim() !== "" && form.sshUser.trim() !== ""))
  );
};

interface ServerFormDialogProps {
  readonly open: boolean;
  /** Present for edits — secrets start masked. */
  readonly server: ManagedServer | null;
  onOpenChange: (open: boolean) => void;
}

function ServerFormDialog({ open, server, onOpenChange }: ServerFormDialogProps) {
  const create = useCreateServer();
  const update = useUpdateServer();
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  // Re-seed the form each time the dialog opens for a different target.
  const [seeded, setSeeded] = useState<ManagedServer | "add" | null>(null);
  const target = server ?? "add";
  if (seeded !== target) {
    setSeeded(target);
    setForm(server === null ? EMPTY_FORM : formFromServer(server));
  }

  const pending = create.isPending || update.isPending;
  const error = create.error ?? update.error;
  const set = (patch: Partial<FormState>): void => setForm((prev) => ({ ...prev, ...patch }));

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (!formValid(form) || pending) return;
    const input = formToInput(form);
    const done = { onSuccess: () => onOpenChange(false) };
    if (server === null) create.mutate(input, done);
    else update.mutate({ id: server.id, input }, done);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{server === null ? "Add server" : `Edit ${server.label}`}</DialogTitle>
          <DialogDescription>
            A sepia server this node can reach. Credentials are stored encrypted on this machine —
            they never enter your browser.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="flex flex-col gap-3">
          <div className="grid gap-2 sm:grid-cols-[1fr_1fr_6rem]">
            <Input
              placeholder="Label"
              aria-label="Server label"
              value={form.label}
              onChange={(event) => set({ label: event.target.value })}
            />
            <Input
              placeholder="host or https://host:port"
              aria-label="Hostname"
              title="A bare host (http), or a full http(s):// address — an explicit :port there overrides the port field"
              value={form.host}
              onChange={(event) => set({ host: event.target.value })}
            />
            <Input
              placeholder="Port"
              aria-label="Port"
              inputMode="numeric"
              value={form.port}
              onChange={(event) => set({ port: event.target.value })}
            />
          </div>
          <div className="grid gap-2 sm:grid-cols-[8rem_1fr_1fr]">
            <Select
              value={form.authType}
              onValueChange={(value) => set({ authType: value as AuthType })}
            >
              <SelectTrigger aria-label="Auth type">
                {form.authType === "none"
                  ? "No auth"
                  : form.authType === "token"
                    ? "Token"
                    : "Password"}
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">No auth</SelectItem>
                <SelectItem value="token">Token</SelectItem>
                <SelectItem value="password">Password</SelectItem>
              </SelectContent>
            </Select>
            {form.authType === "password" && (
              <Input
                placeholder="Username"
                aria-label="Auth username"
                value={form.authUser}
                onChange={(event) => set({ authUser: event.target.value })}
              />
            )}
            {form.authType !== "none" && (
              <Input
                type="password"
                placeholder={form.authType === "token" ? "Bearer token" : "Password"}
                aria-label="Auth secret"
                autoComplete="new-password"
                value={form.authSecret}
                onChange={(event) => set({ authSecret: event.target.value })}
                className={form.authType === "token" ? "sm:col-span-2" : ""}
              />
            )}
          </div>
          <div className="flex items-center gap-3 rounded-lg border border-border px-3 py-2.5">
            <div className="flex-1">
              <span className="block text-sm">SSH tunnel</span>
              <span className="block text-xs text-muted-foreground">
                Reach this server through an SSH local port-forward.
              </span>
            </div>
            <Switch
              checked={form.sshEnabled}
              onCheckedChange={(value) => set({ sshEnabled: value })}
            />
          </div>
          {form.sshEnabled && (
            <>
              <div className="grid gap-2 sm:grid-cols-[1fr_5rem_1fr]">
                <Input
                  placeholder="SSH host"
                  aria-label="SSH host"
                  value={form.sshHost}
                  onChange={(event) => set({ sshHost: event.target.value })}
                />
                <Input
                  placeholder="22"
                  aria-label="SSH port"
                  inputMode="numeric"
                  value={form.sshPort}
                  onChange={(event) => set({ sshPort: event.target.value })}
                />
                <Input
                  placeholder="SSH user"
                  aria-label="SSH user"
                  value={form.sshUser}
                  onChange={(event) => set({ sshUser: event.target.value })}
                />
              </div>
              <Textarea
                placeholder="Key path (~/.ssh/id_ed25519) or paste a private key"
                aria-label="SSH key"
                rows={3}
                className="font-mono text-xs"
                value={form.sshKey}
                onChange={(event) => set({ sshKey: event.target.value })}
              />
            </>
          )}
          {error !== null && (
            <p className="text-xs text-destructive">
              {error instanceof Error ? error.message : "Couldn't save the server"}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" variant="secondary" disabled={!formValid(form) || pending}>
              {pending ? "Saving…" : server === null ? "Add server" : "Save"}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Settings → Servers: the managed-server registry stored on this node
 * (encrypted at rest), separate from the browser-local peer list in Nodes.
 * SSH-configured entries reach their API through a server-side `ssh -L`
 * forward; the status dot probes GET /api/node through the proxy.
 */
export function ServersSection() {
  const { data: servers = [] } = useServers();
  const statuses = useServerStatuses(servers);
  const remove = useDeleteServer();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<ManagedServer | null>(null);
  const [removing, setRemoving] = useState<ManagedServer | null>(null);

  const openAdd = (): void => {
    setEditing(null);
    setDialogOpen(true);
  };
  const openEdit = (server: ManagedServer): void => {
    setEditing(server);
    setDialogOpen(true);
  };

  return (
    <section data-spy="servers" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Servers</h3>
      <p className="text-xs text-muted-foreground">
        Sepia servers this machine can manage — including ones only reachable over SSH. Credentials
        are encrypted on this machine and masked everywhere else.
      </p>
      {servers.length > 0 && (
        <div className="divide-y divide-border/50 rounded-lg border border-border">
          {servers.map((server, index) => (
            <div key={server.id} className="flex items-center gap-3 px-3 py-2.5">
              <StatusDot ok={statuses[index]} />
              <div className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">{server.label}</span>
                <span className="block truncate text-xs text-muted-foreground">
                  {server.scheme}://{server.host}:{server.port}
                  {server.ssh !== null && ` — via ssh ${server.ssh.user}@${server.ssh.host}`}
                  {server.auth !== null &&
                    ` — ${server.auth.type === "token" ? "token" : "password"} ${server.auth.secret}`}
                </span>
              </div>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Edit server ${server.label}`}
                title="Edit server"
                onClick={() => openEdit(server)}
              >
                <HugeiconsIcon icon={PencilEdit01Icon} strokeWidth={2} />
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Remove server ${server.label}`}
                title="Remove server"
                onClick={() => setRemoving(server)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </div>
          ))}
        </div>
      )}
      <div>
        <Button variant="secondary" size="xs" onClick={openAdd}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
          Add server
        </Button>
      </div>
      <ServerFormDialog open={dialogOpen} server={editing} onOpenChange={setDialogOpen} />
      <AlertDialog open={removing !== null} onOpenChange={(open) => !open && setRemoving(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove server?</AlertDialogTitle>
            <AlertDialogDescription>
              {`"${removing?.label ?? ""}" and its stored credentials will be removed from this machine. Any open SSH tunnel is closed.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (removing !== null) remove.mutate(removing.id);
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
