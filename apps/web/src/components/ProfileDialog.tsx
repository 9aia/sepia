import { useUserInfo } from "../hooks/query/useUserInfo";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";

function Row({
  label,
  value,
  mono,
}: {
  readonly label: string;
  readonly value: string;
  readonly mono?: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-4 py-2 text-sm">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span
        className={`min-w-0 truncate text-right ${mono === true ? "font-mono text-xs" : ""}`}
        title={value}
      >
        {value}
      </span>
    </div>
  );
}

interface ProfileDialogProps {
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ProfileDialog({ open, onOpenChange }: ProfileDialogProps) {
  const { data: user, isLoading } = useUserInfo();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Profile</DialogTitle>
          <DialogDescription>
            The OS account the Sepia server runs as — sessions execute with these permissions.
          </DialogDescription>
        </DialogHeader>
        {isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
        {user !== undefined && (
          <div className="divide-y divide-border/50">
            <Row label="Username" value={user.username} />
            <Row label="Host" value={user.hostname} />
            <Row label="Home" value={user.homedir} mono />
            <Row label="Shell" value={user.shell ?? "unknown"} mono />
            <Row label="Platform" value={`${user.platform} (${user.arch})`} />
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
