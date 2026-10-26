import { Banknote, MousePointerClick, ReceiptText, Wallet } from "lucide-react";
import { formatBps, formatCents } from "@/lib/admin";
import type { Summary } from "@/lib/analytics";
import { StatCard } from "./stat-card";

const count = new Intl.NumberFormat("en-US");
export const formatCount = (n: number) => count.format(n);

/** Clicks, conversions, revenue and commission for a range. `currency` null = mixed: money is not summed. */
export function MetricCards({ summary, currency, suffix }: { summary?: Summary; currency: string | null; suffix: string }) {
  const money = (cents?: number) => (summary && currency !== null ? formatCents(cents ?? 0, currency) : "—");
  const mixed = currency === null && summary ? "Mixed currencies: pick a program" : undefined;
  return (
    <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
      <StatCard label={`Clicks (${suffix})`} value={summary ? formatCount(summary.clicks) : "—"} icon={MousePointerClick} />
      <StatCard
        label={`Conversions (${suffix})`}
        value={summary ? formatCount(summary.conversions) : "—"}
        hint={summary ? `${formatBps(summary.conversion_rate_bps)} of clicks` : undefined}
        icon={ReceiptText}
      />
      <StatCard label={`Revenue (${suffix})`} value={money(summary?.revenue_cents)} hint={mixed} icon={Banknote} />
      <StatCard label={`Commission (${suffix})`} value={money(summary?.commission_cents)} hint={mixed} icon={Wallet} accent />
    </div>
  );
}
