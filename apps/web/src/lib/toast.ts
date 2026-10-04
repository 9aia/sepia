import { toast } from "sonner";

export const toastSuccess = (message: string): void => {
  toast.success(message);
};

/** Error toast — pulls a readable message out of whatever was thrown. */
export const toastError = (fallback: string, error?: unknown): void => {
  const detail = error instanceof Error && error.message !== "" ? error.message : undefined;
  toast.error(fallback, { description: detail });
};
