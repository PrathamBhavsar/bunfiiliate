import { cn } from "@/lib/utils";

type Tone = "neutral" | "success" | "warning" | "danger" | "accent";

const dot: Record<Tone, string> = {
  neutral: "bg-muted",
  success: "bg-success",
  warning: "bg-warning",
  danger: "bg-danger",
  accent: "bg-accent",
};

// Domain statuses from the schema: conversions, payouts, affiliates, webhook deliveries.
const tones: Record<string, Tone> = {
  active: "success",
  approved: "success",
  paid: "success",
  delivered: "success",
  pending: "warning",
  draft: "neutral",
  cancelled: "neutral",
  suspended: "danger",
  rejected: "danger",
  reversed: "danger",
  failed: "danger",
  dead: "danger",
};

/** Status chip. Color never carries meaning alone: the label is always shown. */
export function StatusBadge({ status, tone }: { status: string; tone?: Tone }) {
  const t = tone ?? tones[status] ?? "neutral";
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-border bg-surface px-2 py-0.5 text-xs capitalize">
      <span aria-hidden className={cn("size-1.5 rounded-full", dot[t])} />
      {status.replaceAll("_", " ")}
    </span>
  );
}
