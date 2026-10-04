import type { PermissionRequest } from "../lib/types";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";

interface ApprovalDialogProps {
  request: PermissionRequest | null;
  onResolve: (optionId: string | null) => void;
}

export function ApprovalDialog({ request, onResolve }: ApprovalDialogProps) {
  return (
    <Dialog
      open={request !== null}
      onOpenChange={(open) => {
        if (!open) onResolve(null);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{request?.title ?? "Permission requested"}</DialogTitle>
          <DialogDescription>The agent is waiting for your decision.</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-2">
          {request?.options.map((option) => (
            <Button
              key={option.optionId}
              variant="outline"
              className="justify-between"
              onClick={() => onResolve(option.optionId)}
            >
              <span>{option.label}</span>
              {option.kind && <Badge variant="secondary">{option.kind}</Badge>}
            </Button>
          ))}
        </div>
        <DialogFooter>
          <DialogClose render={<Button variant="ghost" />}>Cancel</DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
