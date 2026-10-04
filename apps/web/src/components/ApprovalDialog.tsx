import type { FormEvent } from "react";
import type { PermissionRequest } from "../lib/types";
import { Badge } from "./ui/badge";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import {
  Questionnaire,
  QuestionnaireActions,
  QuestionnaireChoice,
  QuestionnaireChoices,
  QuestionnaireError,
  QuestionnaireItem,
  QuestionnaireNext,
  QuestionnairePrevious,
  QuestionnaireProgress,
  QuestionnaireSubmit,
  QuestionnaireTitle,
} from "./ui/questionnaire";

interface ApprovalDialogProps {
  readonly requests: PermissionRequest[];
  onResolve: (answers: Readonly<Record<string, string>>) => void;
  onCancel: () => void;
}

export function ApprovalDialog({ requests, onResolve, onCancel }: ApprovalDialogProps) {
  const handleSubmit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const answers: Record<string, string> = {};
    for (const request of requests) {
      const value = data.get(request.requestId);
      if (typeof value === "string") answers[request.requestId] = value;
    }
    onResolve(answers);
  };

  return (
    <Dialog
      open={requests.length > 0}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Permission requested</DialogTitle>
          <DialogDescription>The agent is waiting for your decision.</DialogDescription>
        </DialogHeader>
        <Questionnaire
          // Remount per batch so the engine doesn't carry answers between queues.
          key={requests[0]?.requestId ?? "empty"}
          shortcuts="numbers"
          onSubmit={handleSubmit}
        >
          <QuestionnaireProgress />
          {requests.map((request) => (
            <QuestionnaireItem key={request.requestId} name={request.requestId} required>
              <QuestionnaireTitle>{request.title}</QuestionnaireTitle>
              <QuestionnaireChoices>
                {request.options.map((option) => (
                  <QuestionnaireChoice key={option.optionId} value={option.optionId}>
                    <span className="flex w-full items-center justify-between gap-2">
                      {option.label}
                      {option.kind !== undefined && option.kind !== null && (
                        <Badge variant="secondary">{option.kind}</Badge>
                      )}
                    </span>
                  </QuestionnaireChoice>
                ))}
              </QuestionnaireChoices>
            </QuestionnaireItem>
          ))}
          <QuestionnaireError />
          <QuestionnaireActions>
            <QuestionnairePrevious />
            <QuestionnaireNext />
            <QuestionnaireSubmit>Answer</QuestionnaireSubmit>
          </QuestionnaireActions>
        </Questionnaire>
      </DialogContent>
    </Dialog>
  );
}
