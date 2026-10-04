import { Skeleton } from "../ui/skeleton";

/** Mirrors the sectioned list — header rhythm + row shapes while sessions load. */
export function SessionTreeSkeleton() {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col gap-1"
      aria-busy="true"
      aria-label="Loading sessions"
    >
      {[
        { rows: 3, widths: ["w-4/5", "w-3/5", "w-2/3"] },
        { rows: 4, widths: ["w-3/5", "w-4/5", "w-1/2", "w-3/4"] },
      ].map((section, si) => (
        <section key={si}>
          {/* matches SectionHeader: px-3 pt-5 pb-1 + text-sm muted label */}
          <div className="flex items-center justify-between px-3 pt-5 pb-1">
            <Skeleton className="h-4 w-1/4" />
          </div>
          {/* matches SectionRows: px-1.5 gap-1 pb-2 */}
          <div className="flex flex-col gap-1 px-1.5 pb-2">
            {section.widths.map((width, ri) => (
              <div
                key={ri}
                className="flex items-center gap-2 rounded-md px-2 py-1.5"
                aria-hidden="true"
              >
                <Skeleton className={`h-4 ${width}`} />
                <Skeleton className="ml-auto h-3.5 w-10 shrink-0" />
              </div>
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
