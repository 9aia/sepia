import { useState, type KeyboardEvent } from "react";
import { useForm } from "@tanstack/react-form";
import { useStore } from "@tanstack/react-store";
import { Delete02Icon, PencilEdit01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  useAddNode,
  useNodes,
  useNodeStatuses,
  usePairNode,
  useRemoveNode,
  useSelfNode,
  useSetNodeEnabled,
  useUpdateNode,
} from "../hooks/query/useNodes";
import {
  buildPeerFromForm,
  isLocalAccess,
  isPeerEnabled,
  peerUrlParts,
  setPeerAlias,
  type PeerNode,
} from "../lib/nodes";
import { parseServerHost, SECRET_MASK } from "../lib/servers";
import { setSettings, settingsStore } from "../lib/settings";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import { Input } from "./ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "./ui/select";
import { Switch } from "./ui/switch";

/** Reachability dot — undefined while the first probe is in flight. */
function StatusDot({ ok, title }: { readonly ok: boolean | undefined; readonly title?: string }) {
  const color =
    ok === undefined ? "bg-muted-foreground/40" : ok ? "bg-emerald-500" : "bg-destructive";
  return (
    <span
      className={`inline-block size-2 shrink-0 rounded-full ${color}`}
      title={title ?? (ok === undefined ? "Checking…" : ok ? "Reachable" : "Unreachable")}
    />
  );
}

// --- Field validators --------------------------------------------------------
// Every validator returns a message or undefined; fields validate onChange
// (live re-check while fixing) and onSubmit (untouched fields report too).

const hostValidator = ({ value }: { value: string }): string | undefined => {
  if (value.trim() === "") return "Host is required";
  if (parseServerHost(value) === null) return "Enter a hostname or an http(s) address";
  return undefined;
};

const portValidator = ({ value }: { value: string }): string | undefined => {
  const port = Number(value);
  return Number.isInteger(port) && port >= 1 && port <= 65535
    ? undefined
    : "Port must be a number from 1 to 65535";
};

const requiredValidator =
  (message: string) =>
  ({ value }: { value: string }): string | undefined =>
    value.trim() === "" ? message : undefined;

/** Inline error under a field — empty until a validator has complained. */
const fieldError = (errors: ReadonlyArray<unknown>): string | null => {
  const first = errors.find((error): error is string => typeof error === "string" && error !== "");
  return first ?? null;
};

/** Inline error under a field — renders nothing until a validator complains. */
function FieldError({ errors }: { readonly errors: ReadonlyArray<unknown> }) {
  const message = fieldError(errors);
  return message === null ? null : <span className="text-xs text-destructive">{message}</span>;
}

/** The split address fields — shared by the add form and the edit dialog. */
interface AddressFields {
  scheme: "http" | "https";
  host: string;
  port: string;
}

// --- Add form ----------------------------------------------------------------

interface AddFormValues extends AddressFields {
  label: string;
  code: string;
  token: string;
  viaGateway: boolean;
}

const ADD_FORM_DEFAULTS: AddFormValues = {
  label: "",
  scheme: "http",
  host: "",
  port: "8787",
  code: "",
  token: "",
  viaGateway: false,
};

/**
 * Bottom-of-section add form (TanStack Form): label + scheme/host/port
 * address fields, auth via pairing code or bearer token, and the gateway
 * switch. The auth-mode toggle is presentational React state — the code
 * field's validator reads it — while every submitted value lives on the
 * form. Submission still goes through `useAddNode`/`usePairNode` — a wrong
 * address or credential fails the probe before the peer can poison the
 * merged lists.
 */
function NodeAddForm() {
  const addNode = useAddNode();
  const pairNode = usePairNode();
  const [mode, setMode] = useState<"code" | "token">("code");
  const active = mode === "code" ? pairNode : addNode;
  const form = useForm({
    defaultValues: ADD_FORM_DEFAULTS,
    onSubmit: ({ value }) => {
      let url: string;
      try {
        url = buildPeerFromForm(value);
      } catch {
        return; // Field validators normally catch this first.
      }
      const via = value.viaGateway ? ("gateway" as const) : ("direct" as const);
      const onSuccess = (peer: PeerNode): void => {
        if (value.label.trim() !== "") setPeerAlias(peer.id, value.label);
        form.reset();
      };
      if (mode === "code") {
        pairNode.mutate({ url, code: value.code, via }, { onSuccess });
      } else {
        addNode.mutate({ url, token: value.token, via }, { onSuccess });
      }
    },
  });

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
      className="flex flex-col gap-2"
    >
      <div className="flex gap-1">
        <Button
          type="button"
          variant={mode === "code" ? "secondary" : "ghost"}
          size="xs"
          onClick={() => setMode("code")}
        >
          Pairing code
        </Button>
        <Button
          type="button"
          variant={mode === "token" ? "secondary" : "ghost"}
          size="xs"
          onClick={() => setMode("token")}
        >
          Token
        </Button>
      </div>
      <form.Field name="label">
        {(field) => (
          <Input
            placeholder="Nickname (optional)"
            aria-label="Node nickname"
            value={field.state.value}
            onBlur={field.handleBlur}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      <div className="grid gap-2 sm:grid-cols-[5.5rem_1fr_5.5rem]">
        <form.Field name="scheme">
          {(field) => (
            <Select
              value={field.state.value}
              onValueChange={(value) => field.handleChange(value as "http" | "https")}
            >
              <SelectTrigger aria-label="Scheme">{field.state.value}://</SelectTrigger>
              <SelectContent>
                <SelectItem value="http">http://</SelectItem>
                <SelectItem value="https">https://</SelectItem>
              </SelectContent>
            </Select>
          )}
        </form.Field>
        <form.Field name="host" validators={{ onChange: hostValidator, onSubmit: hostValidator }}>
          {(field) => (
            <div className="flex flex-col gap-1">
              <Input
                placeholder="hostname or https://host:port"
                aria-label="Node host"
                title="A bare host, or a full http(s):// address — a scheme or :port it carries wins over the other fields"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
              />
              <FieldError errors={field.state.meta.errors} />
            </div>
          )}
        </form.Field>
        <form.Field name="port" validators={{ onChange: portValidator, onSubmit: portValidator }}>
          {(field) => (
            <div className="flex flex-col gap-1">
              <Input
                placeholder="Port"
                aria-label="Node port"
                inputMode="numeric"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
              />
              <FieldError errors={field.state.meta.errors} />
            </div>
          )}
        </form.Field>
      </div>
      {mode === "code" ? (
        <form.Field
          name="code"
          validators={{
            onChange: requiredValidator("Pairing code is required"),
            onSubmit: requiredValidator("Pairing code is required"),
          }}
        >
          {(field) => (
            <div className="flex flex-col gap-1">
              <Input
                placeholder="Code from `sepia pair` (e.g. 7K2M-9PQX)"
                aria-label="Pairing code"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
              />
              <FieldError errors={field.state.meta.errors} />
            </div>
          )}
        </form.Field>
      ) : (
        // The gateway flag swaps the placeholder — subscribe just this field.
        <form.Subscribe selector={(state) => state.values.viaGateway}>
          {(viaGateway) => (
            <form.Field name="token">
              {(field) => (
                <Input
                  type="password"
                  placeholder={
                    viaGateway ? "Bearer token (stored on this node)" : "Bearer token (if required)"
                  }
                  aria-label="Node token"
                  autoComplete="new-password"
                  value={field.state.value}
                  onBlur={field.handleBlur}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
              )}
            </form.Field>
          )}
        </form.Subscribe>
      )}
      <form.Field name="viaGateway">
        {(field) => (
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
            <Switch
              checked={field.state.value}
              onCheckedChange={(value) => field.handleChange(value)}
              aria-label="Route through this node"
            />
            Route through this node (gateway) — for peers the browser can't reach directly
          </label>
        )}
      </form.Field>
      {active.isError && (
        <p className="text-xs text-destructive">
          {active.error instanceof Error ? active.error.message : "Couldn't reach that node"}
        </p>
      )}
      <form.Subscribe selector={(state) => state.canSubmit}>
        {(canSubmit) => (
          <Button type="submit" variant="secondary" disabled={!canSubmit || active.isPending}>
            {active.isPending ? "Checking…" : mode === "code" ? "Pair node" : "Add node"}
          </Button>
        )}
      </form.Subscribe>
    </form>
  );
}

// --- Edit dialog -------------------------------------------------------------

interface EditFormValues extends AddressFields {
  label: string;
  /** Seeded with SECRET_MASK when a credential exists — the mask means "keep". */
  token: string;
}

const editDefaults = (peer: PeerNode): EditFormValues => {
  const { scheme, host, port } = peerUrlParts(peer.url);
  return {
    label: peer.alias ?? "",
    scheme,
    host,
    port: String(port),
    // Gateway peers keep no token in the browser — the mask still seeds the
    // field and round-trips to "keep the stored credential" on save.
    token: peer.via === "gateway" ? SECRET_MASK : peer.token === null ? "" : SECRET_MASK,
  };
};

/**
 * Per-peer edit form, seeded from the row's peer (keyed remount on id).
 * Saves through `updatePeerEntry`: a direct peer updates in the browser
 * registry; a gateway peer's url/auth changes PATCH its managed-server entry
 * so the stored credential rides along. Label commits via `setPeerAlias` on
 * success.
 */
function NodeEditForm({ peer, onClose }: { readonly peer: PeerNode; onClose: () => void }) {
  const update = useUpdateNode();
  const form = useForm({
    defaultValues: editDefaults(peer),
    onSubmit: ({ value }) => {
      let url: string;
      try {
        url = buildPeerFromForm(value);
      } catch {
        return;
      }
      update.mutate(
        { id: peer.id, update: { url, token: value.token } },
        {
          onSuccess: () => {
            setPeerAlias(peer.id, value.label);
            onClose();
          },
        },
      );
    },
  });

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
      className="flex flex-col gap-3"
    >
      <form.Field name="label">
        {(field) => (
          <Input
            placeholder={peer.name}
            aria-label="Node nickname"
            title="Nickname — shown instead of the node's name; empty reverts to the reported name"
            value={field.state.value}
            onBlur={field.handleBlur}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      <div className="grid gap-2 sm:grid-cols-[5.5rem_1fr_5.5rem]">
        <form.Field name="scheme">
          {(field) => (
            <Select
              value={field.state.value}
              onValueChange={(value) => field.handleChange(value as "http" | "https")}
            >
              <SelectTrigger aria-label="Scheme">{field.state.value}://</SelectTrigger>
              <SelectContent>
                <SelectItem value="http">http://</SelectItem>
                <SelectItem value="https">https://</SelectItem>
              </SelectContent>
            </Select>
          )}
        </form.Field>
        <form.Field name="host" validators={{ onChange: hostValidator, onSubmit: hostValidator }}>
          {(field) => (
            <div className="flex flex-col gap-1">
              <Input
                placeholder="hostname or https://host:port"
                aria-label="Node host"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
              />
              <FieldError errors={field.state.meta.errors} />
            </div>
          )}
        </form.Field>
        <form.Field name="port" validators={{ onChange: portValidator, onSubmit: portValidator }}>
          {(field) => (
            <div className="flex flex-col gap-1">
              <Input
                placeholder="Port"
                aria-label="Node port"
                inputMode="numeric"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => field.handleChange(event.target.value)}
              />
              <FieldError errors={field.state.meta.errors} />
            </div>
          )}
        </form.Field>
      </div>
      <form.Field name="token">
        {(field) => (
          <Input
            type="password"
            placeholder={
              peer.via === "gateway"
                ? "Bearer token (stored on this node — clear to remove)"
                : "Bearer token — clear for none"
            }
            aria-label="Node token"
            autoComplete="new-password"
            value={field.state.value}
            onBlur={field.handleBlur}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      {peer.via === "gateway" && (
        <p className="text-xs text-muted-foreground">
          Calls route through this node — SSH tunnel settings live under Settings → Servers.
        </p>
      )}
      {update.isError && (
        <p className="text-xs text-destructive">
          {update.error instanceof Error ? update.error.message : "Couldn't save the node"}
        </p>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <form.Subscribe selector={(state) => state.canSubmit}>
          {(canSubmit) => (
            <Button type="submit" variant="secondary" disabled={!canSubmit || update.isPending}>
              {update.isPending ? "Saving…" : "Save"}
            </Button>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}

// --- Section -----------------------------------------------------------------

const blurOnEnter = (event: KeyboardEvent<HTMLInputElement>): void => {
  if (event.key === "Enter") event.currentTarget.blur();
};

/**
 * Settings → Nodes: the peer registry behind the federated lists. Each row
 * shows reachability, the display name and address, an enable switch
 * (disabled peers merge nothing and resolve to an unreachable target), and
 * pencil/remove actions — the pencil opens the edit dialog for label,
 * address and credential.
 */
export function NodesSection() {
  const { self, selfStatus, peers } = useNodes();
  const localName = useStore(settingsStore, (s) => s.localNodeName);
  // Drives refreshSelf — populates self, selfStatus and the node alias.
  useSelfNode();
  const statuses = useNodeStatuses(peers);
  const removeNode = useRemoveNode();
  const setEnabled = useSetNodeEnabled();
  const [editing, setEditing] = useState<PeerNode | null>(null);

  return (
    <section data-spy="nodes" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Nodes</h3>
      <p className="text-xs text-muted-foreground">
        Other machines running <code>sepia serve</code>. Their sessions, projects and chat merge
        into this UI — actions go to the machine that owns each session.
      </p>
      <div className="divide-y divide-border/50 rounded-lg border border-border">
        <div className="flex items-center gap-3 px-3 py-2.5">
          <StatusDot
            ok={selfStatus === "unknown" ? undefined : selfStatus === "online"}
            title={
              selfStatus === "offline" ? "Offline — start `sepia serve` on this machine" : undefined
            }
          />
          <div className="min-w-0 flex-1">
            <Input
              key={localName ?? self?.name ?? ""}
              className="h-7 w-44 text-sm font-medium"
              defaultValue={localName ?? ""}
              placeholder={self?.name ?? "This machine"}
              aria-label="Nickname for this machine"
              title="Nickname for this machine"
              onBlur={(e) => setSettings({ localNodeName: e.currentTarget.value.trim() || null })}
              onKeyDown={blurOnEnter}
            />
            <span className="flex items-center gap-1.5 truncate text-xs text-muted-foreground">
              <span className="truncate">{location.origin}</span>
              {isLocalAccess() && (
                <Badge variant="secondary" className="h-4 shrink-0 px-1.5 text-[10px]">
                  this machine
                </Badge>
              )}
            </span>
          </div>
        </div>
        {peers.map((peer, index) => {
          const enabled = isPeerEnabled(peer);
          return (
            <div key={peer.id} className="flex items-center gap-3 px-3 py-2.5">
              <StatusDot ok={statuses[index]} title={enabled ? undefined : "Disabled"} />
              <div className={`min-w-0 flex-1${enabled ? "" : " opacity-60"}`}>
                <span className="block truncate text-sm font-medium">
                  {peer.alias ?? peer.name}
                </span>
                <span className="block truncate text-xs text-muted-foreground">
                  {peer.alias !== undefined ? `${peer.name} — ` : ""}
                  {peer.url}
                  {peer.via === "gateway" && " — via gateway"}
                  {peer.token !== null && ` — token ${SECRET_MASK}`}
                  {!enabled && " — disabled"}
                </span>
              </div>
              <Switch
                checked={enabled}
                onCheckedChange={(value) => setEnabled.mutate({ id: peer.id, enabled: value })}
                aria-label={`${enabled ? "Disable" : "Enable"} node ${peer.name}`}
                title={enabled ? "Disable node" : "Enable node"}
              />
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Edit node ${peer.name}`}
                title="Edit node"
                onClick={() => setEditing(peer)}
              >
                <HugeiconsIcon icon={PencilEdit01Icon} strokeWidth={2} />
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Remove node ${peer.name}`}
                title="Remove node"
                onClick={() => removeNode.mutate(peer.id)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </div>
          );
        })}
      </div>
      <NodeAddForm />
      <Dialog open={editing !== null} onOpenChange={(open) => !open && setEditing(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{`Edit ${editing?.alias ?? editing?.name ?? "node"}`}</DialogTitle>
            <DialogDescription>
              Address and credential for this peer. The masked token keeps the stored value — clear
              it for no credential.
            </DialogDescription>
          </DialogHeader>
          {editing !== null && (
            <NodeEditForm key={editing.id} peer={editing} onClose={() => setEditing(null)} />
          )}
        </DialogContent>
      </Dialog>
    </section>
  );
}
