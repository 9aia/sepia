import { useEffect, useRef } from "react";
import { useHotkey } from "@tanstack/react-hotkeys";
import type { PermissionRequest } from "../lib/types";

interface ApprovalDialogProps {
  request: PermissionRequest | null;
  onResolve: (optionId: string | null) => void;
}

export function ApprovalDialog({ request, onResolve }: ApprovalDialogProps) {
  const dialogRef = useRef<HTMLDivElement | null>(null);

  useHotkey("Escape", () => onResolve(null), { enabled: request !== null });

  useEffect(() => {
    if (request) dialogRef.current?.querySelector("button")?.focus();
  }, [request]);

  if (!request) return null;

  return (
    <div className="approval-overlay" role="dialog" aria-modal="true">
      <div className="approval-dialog" ref={dialogRef}>
        <h2 className="approval-dialog__title">{request.title}</h2>
        <p className="approval-dialog__hint">The agent is waiting for your decision.</p>
        <div className="approval-dialog__options">
          {request.options.map((option) => (
            <button
              key={option.optionId}
              type="button"
              className="approval-dialog__option"
              onClick={() => onResolve(option.optionId)}
            >
              <span>{option.label}</span>
              {option.kind && <span className="approval-dialog__kind">{option.kind}</span>}
            </button>
          ))}
        </div>
        <button type="button" className="approval-dialog__cancel" onClick={() => onResolve(null)}>
          Cancel
        </button>
      </div>
    </div>
  );
}
