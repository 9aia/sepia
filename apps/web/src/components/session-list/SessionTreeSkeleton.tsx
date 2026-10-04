import { Skeleton } from "../ui/skeleton";

/** Mirrors the tree's group→item rhythm while sessions load. */
export function SessionTreeSkeleton() {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col p-2"
      aria-busy="true"
      aria-label="Loading sessions"
    >
      <div className="border-b border-border/50 px-2 py-2">
        <Skeleton className="h-3.5 w-2/5" />
      </div>
      {Array.from({ length: 6 }).map((_, i) => (
        <div key={i} className="border-b border-border/50 px-2 py-1.5">
          <div className="flex flex-col gap-1.5 py-1">
            <Skeleton className={`h-3.5 ${i % 2 === 0 ? "w-4/5" : "w-3/5"}`} />
            <Skeleton className="h-3 w-1/3" />
          </div>
        </div>
      ))}
      <div className="border-b border-border/50 px-2 py-2">
        <Skeleton className="h-3.5 w-1/3" />
      </div>
      {Array.from({ length: 3 }).map((_, i) => (
        <div key={i} className="border-b border-border/50 px-2 py-1.5">
          <div className="flex flex-col gap-1.5 py-1">
            <Skeleton className="h-3.5 w-3/4" />
            <Skeleton className="h-3 w-1/4" />
          </div>
        </div>
      ))}
    </div>
  );
}
