import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** One headline number. `accent` paints it orange; use it for the key figure on a page. */
export function StatCard({
  label,
  value,
  hint,
  icon: Icon,
  accent,
}: {
  label: string;
  value: ReactNode;
  hint?: string;
  icon?: LucideIcon;
  accent?: boolean;
}) {
  return (
    <div className="rounded-lg border border-border bg-surface p-3">
      <div className="flex items-center justify-between text-xs text-muted">
        <span>{label}</span>
        {Icon && <Icon aria-hidden className="size-3.5" />}
      </div>
      <div className={cn("mt-1 text-2xl font-semibold tabular-nums", accent && "text-accent-text")}>{value}</div>
      {hint && <div className="mt-0.5 text-xs text-muted">{hint}</div>}
    </div>
  );
}
