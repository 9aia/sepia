import { toast } from "sonner";

export const toastSuccess = (message: string, id?: string | number): void => {
  if (id === undefined) toast.success(message);
  else toast.success(message, { id });
};

/** Pending toast for long mutations — resolve it via the `id` param of the
 *  success/error variants so the spinner morphs instead of stacking. */
export const toastLoading = (message: string): string | number => toast.loading(message);

/** Error toast — pulls a readable message out of whatever was thrown. */
export const toastError = (fallback: string, error?: unknown, id?: string | number): void => {
  const detail = error instanceof Error && error.message !== "" ? error.message : undefined;
  toast.error(fallback, {
    description: detail,
    ...(id === undefined ? {} : { id }),
  });
};
