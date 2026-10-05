import { useState, type ReactNode } from "react";
import { useForm } from "@tanstack/react-form";
import { useStore } from "@tanstack/react-store";
import {
  Copy01Icon,
  Delete02Icon,
  MonitorIcon,
  MoreVerticalIcon,
  PencilEdit01Icon,
} from "@hugeicons/core-free-icons";
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
  isLocalAccess,
  isPeerEnabled,
  parseNodeAddress,
  setPeerAlias,
  type PeerCredentialSpec,
  type PeerNode,
} from "../lib/nodes";
import { credentialsStore } from "../lib/credentials";
import { LOCAL_NODE_ID } from "../lib/format";
import { SECRET_MASK } from "../lib/servers";
import { setSettings, settingsStore } from "../lib/settings";
import { setSettingsOpen } from "../lib/store";
import { toastSuccess } from "../lib/toast";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "./ui/alert-dialog";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { Input } from "./ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "./ui/select";
import { Switch } from "./ui/switch";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";

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

/** The unified address field — one input for host, host:port or a full URL. */
const addressValidator = ({ value }: { value: string }): string | undefined => {
  const parsed = parseNodeAddress(value);
  return parsed.ok ? undefined : parsed.error;
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

/** Small muted caption above a form control — keeps the card's grid readable. */
function FieldLabel({ children }: { readonly children: ReactNode }) {
  return <span className="text-xs font-medium text-muted-foreground">{children}</span>;
}

/**
 * The unified address field shared by the add form and the edit dialog —
 * one input covering `host`, `host:port` and `http(s)://…` forms. The parsed
 * scheme rides inside the value; there's no separate scheme/port control.
 */
function AddressField({
  field,
}: {
  readonly field: {
    state: { value: string; meta: { errors: ReadonlyArray<unknown> } };
    handleBlur: () => void;
    handleChange: (value: string) => void;
  };
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <FieldLabel>Address</FieldLabel>
      <Input
        placeholder="hostname, host:port or https://…"
        aria-label="Node address"
        title="A bare host (defaults to http://…:8787), a host:port pair, or a full http(s):// address"
        value={field.state.value}
        onBlur={field.handleBlur}
        onChange={(event) => field.handleChange(event.target.value)}
      />
      <FieldError errors={field.state.meta.errors} />
    </div>
  );
}

// --- Credential picker -------------------------------------------------------
// Direct peers link a stored credential (Settings → Credentials) instead of
// holding a raw token — the picker lists the store plus "none"/"new" exits.
// Gateway-routed peers still take a raw secret (it lands in the managed
// registry server-side), so the forms keep the password field for that mode.

const CREDENTIAL_NONE = "__none__";
const CREDENTIAL_NEW = "__new__";

/** Map the picker's value to the spec the node ops take. */
const credentialSpec = (
  picker: string,
  newSecret: string,
  label: string,
): PeerCredentialSpec | null =>
  picker === CREDENTIAL_NONE
    ? null
    : picker === CREDENTIAL_NEW
      ? { secret: newSecret, label: label.trim() === "" ? undefined : label.trim() }
      : { credentialId: picker };

/** Select of stored credentials + "No credential"/"New credential…" exits. */
function CredentialSelect({
  value,
  onChange,
  onBlur,
}: {
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly onBlur?: () => void;
}) {
  const credentials = useStore(credentialsStore);
  const label =
    value === CREDENTIAL_NONE
      ? "No credential"
      : value === CREDENTIAL_NEW
        ? "New credential…"
        : (credentials.find((credential) => credential.id === value)?.label ??
          "Missing credential");
  return (
    <Select value={value} onValueChange={(v) => onChange(v ?? CREDENTIAL_NONE)}>
      <SelectTrigger aria-label="Node credential" className="w-full" onBlur={onBlur}>
        {label}
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={CREDENTIAL_NONE}>No credential</SelectItem>
        {credentials.map((credential) => (
          <SelectItem key={credential.id} value={credential.id}>
            {credential.label}
          </SelectItem>
        ))}
        <SelectItem value={CREDENTIAL_NEW}>New credential…</SelectItem>
      </SelectContent>
    </Select>
  );
}

/**
 * The "new credential" secret field — required only while the picker is on
 * "New credential…" (the field unmounts otherwise, but the check keeps a
 * stale value from blocking a submit).
 */
const newSecretValidator = ({
  value,
  fieldApi,
}: {
  value: string;
  fieldApi: { form: { state: { values: { credential: string } } } };
}): string | undefined =>
  fieldApi.form.state.values.credential === CREDENTIAL_NEW && value.trim() === ""
    ? "Enter the token for the new credential"
    : undefined;

// --- Add form ----------------------------------------------------------------

interface AddFormValues {
  label: string;
  /** Unified address — host, host:port or a full http(s):// URL. */
  host: string;
  code: string;
  /** Direct peers: picker value — "__none__", "__new__", or a credential id. */
  credential: string;
  /** Secret for a "__new__" pick — filed in the credential store on save. */
  newSecret: string;
  /** Gateway peers only: the raw secret submitted to the managed registry. */
  token: string;
  viaGateway: boolean;
}

const ADD_FORM_DEFAULTS: AddFormValues = {
  label: "",
  host: "",
  code: "",
  credential: CREDENTIAL_NONE,
  newSecret: "",
  token: "",
  viaGateway: false,
};

/**
 * Bottom-of-section add form (TanStack Form): nickname + a unified address
 * field, auth via pairing code or a credential (a stored pick, a fresh
 * secret filed in the credential store, or — gateway mode — a raw secret
 * for the managed registry), and the gateway switch. The auth-mode toggle
 * is presentational React state — the code field's validator reads it —
 * while every submitted value lives on the form. Submission still goes
 * through `useAddNode`/`usePairNode` — a wrong address or credential fails
 * the probe before the peer can poison the merged lists.
 */
function NodeAddForm() {
  const addNode = useAddNode();
  const pairNode = usePairNode();
  const [mode, setMode] = useState<"code" | "token">("code");
  const active = mode === "code" ? pairNode : addNode;
  const form = useForm({
    defaultValues: ADD_FORM_DEFAULTS,
    onSubmit: ({ value }) => {
      const parsed = parseNodeAddress(value.host);
      if (!parsed.ok) return; // The field validator normally catches this first.
      const url = parsed.address.url;
      const via = value.viaGateway ? ("gateway" as const) : ("direct" as const);
      const onSuccess = (peer: PeerNode): void => {
        if (value.label.trim() !== "") setPeerAlias(peer.id, value.label);
        form.reset();
      };
      if (mode === "code") {
        pairNode.mutate({ url, code: value.code, via }, { onSuccess });
      } else if (via === "gateway") {
        addNode.mutate({ url, token: value.token, via }, { onSuccess });
      } else {
        addNode.mutate(
          {
            url,
            credential: credentialSpec(value.credential, value.newSecret, value.label),
            via,
          },
          { onSuccess },
        );
      }
    },
  });

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
      className="flex flex-col gap-3 rounded-lg border border-border p-3"
    >
      <div>
        <span className="block text-sm font-medium">Add a node</span>
        <span className="block text-xs text-muted-foreground">
          Pair with a code from <code>sepia pair</code> on that machine, or add it with its bearer
          token.
        </span>
      </div>
      <form.Field name="label">
        {(field) => (
          <div className="flex flex-col gap-1.5">
            <FieldLabel>Nickname · optional</FieldLabel>
            <Input
              placeholder="Shown instead of the node's name"
              aria-label="Node nickname"
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </div>
        )}
      </form.Field>
      <form.Field
        name="host"
        validators={{ onChange: addressValidator, onSubmit: addressValidator }}
      >
        {(field) => <AddressField field={field} />}
      </form.Field>
      <Tabs value={mode} onValueChange={(value) => setMode(value as "code" | "token")}>
        <TabsList className="w-full" aria-label="Auth method">
          <TabsTrigger value="code" className="flex-1">
            Pairing code
          </TabsTrigger>
          <TabsTrigger value="token" className="flex-1">
            Token
          </TabsTrigger>
        </TabsList>
      </Tabs>
      {mode === "code" ? (
        <form.Field
          name="code"
          validators={{
            onChange: requiredValidator("Pairing code is required"),
            onSubmit: requiredValidator("Pairing code is required"),
          }}
        >
          {(field) => (
            <div className="flex flex-col gap-1.5">
              <FieldLabel>Pairing code</FieldLabel>
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
        // Gateway routing keeps the raw-secret field (it goes to the managed
        // registry); direct peers pick a stored credential instead.
        <form.Subscribe
          selector={(state) => [state.values.viaGateway, state.values.credential] as const}
        >
          {([viaGateway, credential]) =>
            viaGateway ? (
              <form.Field name="token">
                {(field) => (
                  <div className="flex flex-col gap-1.5">
                    <FieldLabel>Bearer token</FieldLabel>
                    <Input
                      type="password"
                      placeholder="Stored on this node"
                      aria-label="Node token"
                      autoComplete="new-password"
                      value={field.state.value}
                      onBlur={field.handleBlur}
                      onChange={(event) => field.handleChange(event.target.value)}
                    />
                  </div>
                )}
              </form.Field>
            ) : (
              <div className="flex flex-col gap-1.5">
                <FieldLabel>Credential</FieldLabel>
                <form.Field name="credential">
                  {(field) => (
                    <CredentialSelect
                      value={field.state.value}
                      onChange={field.handleChange}
                      onBlur={field.handleBlur}
                    />
                  )}
                </form.Field>
                {credential === CREDENTIAL_NEW && (
                  <form.Field
                    name="newSecret"
                    validators={{ onChange: newSecretValidator, onSubmit: newSecretValidator }}
                  >
                    {(field) => (
                      <div className="flex flex-col gap-1">
                        <Input
                          type="password"
                          placeholder="Bearer token — filed in Credentials"
                          aria-label="New credential secret"
                          autoComplete="new-password"
                          value={field.state.value}
                          onBlur={field.handleBlur}
                          onChange={(event) => field.handleChange(event.target.value)}
                        />
                        <FieldError errors={field.state.meta.errors} />
                      </div>
                    )}
                  </form.Field>
                )}
              </div>
            )
          }
        </form.Subscribe>
      )}
      <form.Field name="viaGateway">
        {(field) => (
          <div className="flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <span className="block text-sm">Route through this node</span>
              <span className="block text-xs text-muted-foreground">
                Gateway mode — for peers this browser can&apos;t reach directly.
              </span>
            </div>
            <Switch
              checked={field.state.value}
              onCheckedChange={(value) => field.handleChange(value)}
              aria-label="Route through this node"
            />
          </div>
        )}
      </form.Field>
      {active.isError && (
        <p className="text-xs text-destructive">
          {active.error instanceof Error ? active.error.message : "Couldn't reach that node"}
        </p>
      )}
      <div className="flex justify-end">
        <form.Subscribe selector={(state) => state.canSubmit}>
          {(canSubmit) => (
            <Button type="submit" variant="secondary" disabled={!canSubmit || active.isPending}>
              {active.isPending ? "Checking…" : mode === "code" ? "Pair node" : "Add node"}
            </Button>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}

// --- Edit dialog -------------------------------------------------------------

interface EditFormValues {
  label: string;
  /** Unified address — seeded with the peer's canonical origin. */
  host: string;
  /**
   * Direct-peer credential picker — "__none__", "__new__", or a credential
   * id. Seeded with the peer's current link so an untouched save keeps it.
   */
  credential: string;
  /** Secret for a "__new__" pick — filed in the credential store on save. */
  newSecret: string;
  /**
   * Gateway-mode raw secret — seeded with SECRET_MASK, which round-trips to
   * "keep the stored credential" (on a direct → gateway switch it moves the
   * peer's linked credential into the managed registry).
   */
  token: string;
  /** The `via` flag — checked means calls route through this node's server. */
  viaGateway: boolean;
}

const editDefaults = (peer: PeerNode): EditFormValues => ({
  label: peer.alias ?? "",
  // The canonical origin round-trips through parseNodeAddress unchanged.
  host: peer.url,
  credential: peer.credentialId ?? CREDENTIAL_NONE,
  newSecret: "",
  // Gateway peers keep no credential in the browser — the mask still seeds
  // the field and round-trips to "keep the stored credential" on save.
  token: SECRET_MASK,
  viaGateway: peer.via === "gateway",
});

/**
 * The routing switch's subline — explains the current mode and warns on the
 * transition each way: direct → gateway moves the linked credential into
 * this node's store; gateway → direct drops it (the stored secret can't
 * come back), so the picked credential becomes the whole credential.
 */
const viaDescription = (peer: PeerNode, viaGateway: boolean): string => {
  if (viaGateway && peer.via === "gateway") {
    return "Calls route through this node's server — SSH tunnel settings live under Settings → Servers.";
  }
  if (viaGateway) {
    return "Calls route through this node's server — the linked credential moves to its encrypted store.";
  }
  if (peer.via === "gateway") {
    return "Calls go straight from the browser — the stored credential is removed, so pick a credential above if the peer needs one.";
  }
  return "Gateway mode — for peers this browser can't reach directly.";
};

/** Gateway-mode secret field's placeholder — mask means "keep stored". */
const GATEWAY_TOKEN_PLACEHOLDER = "Stored on this node — clear to remove";

/**
 * Per-peer edit form, seeded from the row's peer (keyed remount on id).
 * Saves through `updatePeerEntry`: a direct peer updates in the browser
 * registry; a gateway peer's url/auth changes PATCH its managed-server entry
 * so the stored credential rides along; flipping the routing switch moves
 * the credential between the browser and the node's store. Label commits
 * via `setPeerAlias` on success.
 */
function NodeEditForm({ peer, onClose }: { readonly peer: PeerNode; onClose: () => void }) {
  const update = useUpdateNode();
  const form = useForm({
    defaultValues: editDefaults(peer),
    onSubmit: ({ value }) => {
      const parsed = parseNodeAddress(value.host);
      if (!parsed.ok) return;
      const url = parsed.address.url;
      update.mutate(
        {
          id: peer.id,
          update: {
            url,
            via: value.viaGateway ? ("gateway" as const) : ("direct" as const),
            // Gateway routing submits the raw secret to the managed
            // registry; direct routing relinks/creates a stored credential.
            ...(value.viaGateway
              ? { token: value.token }
              : {
                  credential: credentialSpec(value.credential, value.newSecret, value.label),
                }),
          },
        },
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
          <div className="flex flex-col gap-1.5">
            <FieldLabel>Nickname · optional</FieldLabel>
            <Input
              placeholder={peer.name}
              aria-label="Node nickname"
              title="Nickname — shown instead of the node's name; empty reverts to the reported name"
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </div>
        )}
      </form.Field>
      <form.Field
        name="host"
        validators={{ onChange: addressValidator, onSubmit: addressValidator }}
      >
        {(field) => <AddressField field={field} />}
      </form.Field>
      <form.Subscribe
        selector={(state) => [state.values.viaGateway, state.values.credential] as const}
      >
        {([viaGateway, credential]) =>
          viaGateway ? (
            <form.Field name="token">
              {(field) => (
                <div className="flex flex-col gap-1.5">
                  <FieldLabel>Bearer token</FieldLabel>
                  <Input
                    type="password"
                    placeholder={GATEWAY_TOKEN_PLACEHOLDER}
                    aria-label="Node token"
                    autoComplete="new-password"
                    value={field.state.value}
                    onBlur={field.handleBlur}
                    onChange={(event) => field.handleChange(event.target.value)}
                  />
                </div>
              )}
            </form.Field>
          ) : (
            <div className="flex flex-col gap-1.5">
              <FieldLabel>Credential</FieldLabel>
              <form.Field name="credential">
                {(field) => (
                  <CredentialSelect
                    value={field.state.value}
                    onChange={field.handleChange}
                    onBlur={field.handleBlur}
                  />
                )}
              </form.Field>
              {credential === CREDENTIAL_NEW && (
                <form.Field
                  name="newSecret"
                  validators={{ onChange: newSecretValidator, onSubmit: newSecretValidator }}
                >
                  {(field) => (
                    <div className="flex flex-col gap-1">
                      <Input
                        type="password"
                        placeholder="Bearer token — filed in Credentials"
                        aria-label="New credential secret"
                        autoComplete="new-password"
                        value={field.state.value}
                        onBlur={field.handleBlur}
                        onChange={(event) => field.handleChange(event.target.value)}
                      />
                      <FieldError errors={field.state.meta.errors} />
                    </div>
                  )}
                </form.Field>
              )}
            </div>
          )
        }
      </form.Subscribe>
      <form.Field name="viaGateway">
        {(field) => (
          <div className="flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <span className="block text-sm">Route through this node</span>
              <span className="block text-xs text-muted-foreground">
                {viaDescription(peer, field.state.value)}
              </span>
            </div>
            <Switch
              checked={field.state.value}
              onCheckedChange={(value) => field.handleChange(value)}
              aria-label="Route through this node"
            />
          </div>
        )}
      </form.Field>
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

// --- Local-node edit dialog --------------------------------------------------

/** Copy helper — clipboard APIs can be absent on insecure origins. */
const copy = (value: string): void => {
  void navigator.clipboard?.writeText(value).then(
    () => toastSuccess("Copied to clipboard"),
    () => undefined,
  );
};

/**
 * The serving node's edit form — a slimmer `NodeEditForm`: only the
 * nickname is writable (`localNodeName`, the peer `alias` equivalent). The
 * address is `location.origin` — it's whatever URL the client was opened
 * on, so it can't be edited here — and there's no credential or remove:
 * the serving node is the transport, not an entry to delete. The client
 * keypair row just links to Settings → Client.
 */
function LocalNodeEditForm({
  selfName,
  onClose,
}: {
  readonly selfName: string | undefined;
  readonly onClose: () => void;
}) {
  const localName = useStore(settingsStore, (s) => s.localNodeName);
  const [name, setName] = useState(localName ?? "");
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        setSettings({ localNodeName: name.trim() === "" ? null : name.trim() });
        onClose();
      }}
      className="flex flex-col gap-3"
    >
      <div className="flex flex-col gap-1.5">
        <FieldLabel>Nickname · optional</FieldLabel>
        <Input
          placeholder={selfName ?? "This machine"}
          aria-label="Node nickname"
          title="Nickname — shown instead of the node's name; empty reverts to the reported name"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <FieldLabel>Address</FieldLabel>
        <div className="flex items-center gap-2">
          <Input
            readOnly
            value={location.origin}
            aria-label="Node address"
            title="The node serving this UI — its address is the URL you open the client on"
          />
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label="Copy node address"
            title="Copy node address"
            onClick={() => copy(location.origin)}
          >
            <HugeiconsIcon icon={Copy01Icon} strokeWidth={2} />
          </Button>
        </div>
        <span className="text-xs text-muted-foreground">
          Read-only — this is the URL you access the client on; point peers at it to add this
          machine as a node.
        </span>
      </div>
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <span className="block text-sm">Client identity</span>
          <span className="block text-xs text-muted-foreground">
            This browser&apos;s label and keypair live under Settings → Client.
          </span>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="xs"
          onClick={() => {
            // Jump to the Client section inside the still-open settings dialog.
            setSettingsOpen(true, "client");
            onClose();
          }}
        >
          Open
        </Button>
      </div>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button type="submit" variant="secondary">
          Save
        </Button>
      </div>
    </form>
  );
}

// --- Section -----------------------------------------------------------------

/**
 * Settings → Nodes: the node registry behind the federated lists — the
 * serving node first (same controls as a peer, minus remove), then each
 * registered peer. Each row shows reachability, the display name and
 * address, an enable switch (disabled nodes merge nothing; a disabled peer
 * also resolves to an unreachable target), and an edit affordance — the
 * local row's pencil opens its own dialog (nickname + read-only address),
 * a peer's ⋮ menu covers every peer subfield (nickname, address,
 * credential, routing) plus a confirmed remove since a gateway peer's
 * stored credential dies with it.
 */
export function NodesSection() {
  const { self, selfStatus, peers } = useNodes();
  const credentials = useStore(credentialsStore);
  const localName = useStore(settingsStore, (s) => s.localNodeName);
  const localEnabled = useStore(settingsStore, (s) => s.localNodeEnabled);
  // Drives refreshSelf — populates self, selfStatus and the node alias.
  useSelfNode();
  const statuses = useNodeStatuses(peers);
  const removeNode = useRemoveNode();
  const setEnabled = useSetNodeEnabled();
  const [editing, setEditing] = useState<PeerNode | null>(null);
  const [editingLocal, setEditingLocal] = useState(false);
  const [removing, setRemoving] = useState<PeerNode | null>(null);

  return (
    <section data-spy="nodes" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Nodes</h3>
      <p className="text-xs text-muted-foreground">
        Machines running <code>sepia serve</code>. Their sessions, projects and chat merge into this
        client — actions go to the machine that holds each session's lock.
      </p>
      <div className="divide-y divide-border/50 rounded-lg border border-border">
        <div className="flex items-center gap-3 px-3 py-2.5">
          <StatusDot
            ok={
              localEnabled
                ? selfStatus === "unknown"
                  ? undefined
                  : selfStatus === "online"
                : undefined
            }
            title={
              !localEnabled
                ? "Disabled"
                : selfStatus === "offline"
                  ? `Unreachable — run \`sepia serve\` on ${isLocalAccess() ? "this machine" : location.host}`
                  : undefined
            }
          />
          <div className={`min-w-0 flex-1${localEnabled ? "" : " opacity-60"}`}>
            <span className="flex items-center gap-1.5">
              <span className="truncate text-sm font-medium">
                {localName ?? self?.name ?? (isLocalAccess() ? "This machine" : location.host)}
              </span>
              {isLocalAccess() && (
                <Badge
                  variant="outline"
                  className="shrink-0 text-muted-foreground"
                  title="The node serving this UI"
                >
                  <HugeiconsIcon icon={MonitorIcon} strokeWidth={2} data-icon="inline-start" />
                  this machine
                </Badge>
              )}
              {!localEnabled && (
                <Badge variant="outline" className="h-4 shrink-0 px-1.5 text-[10px]">
                  disabled
                </Badge>
              )}
            </span>
            <span className="block truncate text-xs text-muted-foreground">
              {localName !== null && self !== null ? `${self.name} — ` : ""}
              {location.origin}
            </span>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            <Switch
              checked={localEnabled}
              onCheckedChange={(value) => setEnabled.mutate({ id: LOCAL_NODE_ID, enabled: value })}
              aria-label={`${localEnabled ? "Disable" : "Enable"} node ${self?.name ?? "this machine"}`}
              title={localEnabled ? "Disable node" : "Enable node"}
            />
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Edit this node"
              title="Edit node"
              onClick={() => setEditingLocal(true)}
            >
              <HugeiconsIcon icon={PencilEdit01Icon} strokeWidth={2} />
            </Button>
          </div>
        </div>
        {peers.map((peer, index) => {
          const enabled = isPeerEnabled(peer);
          return (
            <div key={peer.id} className="flex items-center gap-3 px-3 py-2.5">
              <StatusDot ok={statuses[index]} title={enabled ? undefined : "Disabled"} />
              <div className={`min-w-0 flex-1${enabled ? "" : " opacity-60"}`}>
                <span className="flex items-center gap-1.5">
                  <span className="truncate text-sm font-medium">{peer.alias ?? peer.name}</span>
                  {peer.via === "gateway" && (
                    <Badge variant="secondary" className="h-4 shrink-0 px-1.5 text-[10px]">
                      gateway
                    </Badge>
                  )}
                  {!enabled && (
                    <Badge variant="outline" className="h-4 shrink-0 px-1.5 text-[10px]">
                      disabled
                    </Badge>
                  )}
                </span>
                <span className="block truncate text-xs text-muted-foreground">
                  {peer.alias !== undefined ? `${peer.name} — ` : ""}
                  {peer.url}
                  {peer.via !== "gateway" &&
                    peer.credentialId !== undefined &&
                    ` — credential ${credentials.find((credential) => credential.id === peer.credentialId)?.label ?? "missing"} ${SECRET_MASK}`}
                </span>
              </div>
              <div className="flex shrink-0 items-center gap-1.5">
                <Switch
                  checked={enabled}
                  onCheckedChange={(value) => setEnabled.mutate({ id: peer.id, enabled: value })}
                  aria-label={`${enabled ? "Disable" : "Enable"} node ${peer.name}`}
                  title={enabled ? "Disable node" : "Enable node"}
                />
                <DropdownMenu>
                  <DropdownMenuTrigger
                    render={
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        aria-label={`Actions for node ${peer.name}`}
                        title="Node actions"
                      />
                    }
                  >
                    <HugeiconsIcon icon={MoreVerticalIcon} strokeWidth={2} />
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem onClick={() => setEditing(peer)}>
                      <HugeiconsIcon icon={PencilEdit01Icon} strokeWidth={2} />
                      Edit node
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem variant="destructive" onClick={() => setRemoving(peer)}>
                      <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                      Remove node
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </div>
          );
        })}
      </div>
      <NodeAddForm />
      <Dialog open={editingLocal} onOpenChange={setEditingLocal}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{`Edit ${localName ?? self?.name ?? "this node"}`}</DialogTitle>
            <DialogDescription>
              The node serving this UI — only the nickname is editable; its address follows the URL
              you access the client on, and it can&apos;t be removed.
            </DialogDescription>
          </DialogHeader>
          <LocalNodeEditForm selfName={self?.name} onClose={() => setEditingLocal(false)} />
        </DialogContent>
      </Dialog>
      <Dialog open={editing !== null} onOpenChange={(open) => !open && setEditing(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{`Edit ${editing?.alias ?? editing?.name ?? "node"}`}</DialogTitle>
            <DialogDescription>
              Nickname, address, credential and routing for this peer. Direct nodes link a stored
              credential; gateway routing stores the secret on this node (the mask keeps it).
            </DialogDescription>
          </DialogHeader>
          {editing !== null && (
            <NodeEditForm key={editing.id} peer={editing} onClose={() => setEditing(null)} />
          )}
        </DialogContent>
      </Dialog>
      <AlertDialog open={removing !== null} onOpenChange={(open) => !open && setRemoving(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove node?</AlertDialogTitle>
            <AlertDialogDescription>
              {`"${removing?.alias ?? removing?.name ?? ""}" leaves the peer list — its sessions, projects and chat stop merging into this UI.`}
              {removing?.via === "gateway" &&
                " The credential stored on this machine is removed too."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (removing !== null) removeNode.mutate(removing.id);
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
