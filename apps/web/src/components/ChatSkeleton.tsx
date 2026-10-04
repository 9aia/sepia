import { Skeleton } from "./ui/skeleton";

/** Mirrors the chat page while sessions load: header, bubbles, prompt. */
export function ChatSkeleton() {
  return (
    <div className="flex h-svh flex-col overflow-hidden" aria-busy="true" aria-label="Loading chat">
      <div className="flex items-center gap-3 border-b border-border px-4 py-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <Skeleton className="h-4 w-48" />
          <Skeleton className="h-3 w-64" />
        </div>
        <Skeleton className="size-8 rounded-full" />
        <Skeleton className="size-7 rounded-md" />
      </div>

      <div className="mx-auto flex w-full max-w-3xl flex-1 flex-col justify-end gap-5 px-4 pb-6">
        <div className="flex flex-col items-start gap-1.5">
          <Skeleton className="size-7 rounded-full" />
          <Skeleton className="h-10 w-3/4 rounded-2xl" />
          <Skeleton className="h-10 w-1/2 rounded-2xl" />
        </div>
        <div className="flex flex-col items-end gap-1.5">
          <Skeleton className="size-7 rounded-full" />
          <Skeleton className="h-9 w-2/5 rounded-2xl" />
        </div>
        <div className="flex flex-col items-start gap-1.5">
          <Skeleton className="size-7 rounded-full" />
          <Skeleton className="h-10 w-2/3 rounded-2xl" />
        </div>
      </div>

      <div className="border-t border-border px-4 pt-3 pb-4">
        <Skeleton className="h-16 w-full rounded-xl" />
      </div>
    </div>
  );
}
