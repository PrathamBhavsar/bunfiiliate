"use client";

import { ErrorState } from "@/components/state";
import { useQuery } from "@tanstack/react-query";
import { Banknote, MousePointerClick, Percent, ReceiptText } from "lucide-react";
import { StatCard } from "@/components/stat-card";
import { Skeleton } from "@/components/ui/skeleton";
import { formatBps, formatCents, ok, usePrograms } from "@/lib/admin";
import { api } from "@/lib/api";

type Day = { day: string; clicks: number; commission_cents: number };

// Affiliates always get their own figures from the analytics routes; no affiliate_id is sent.
function useOverview() {
  return useQuery({
    queryKey: ["portal", "overview"],
    queryFn: async ({ signal }) => {
      const [summary, days] = await Promise.all([
        ok(api.GET("/api/v1/analytics/summary", { signal })),
        ok(api.GET("/api/v1/analytics/timeseries", { signal })),
      ]);
      return { summary, days };
    },
  });
}

/** One bar per day, scaled to the busiest day. The title on each bar carries the exact value. */
function DayBars({ label, days, value }: { label: string; days: Day[]; value: (d: Day) => { n: number; text: string } }) {
  const max = Math.max(1, ...days.map((d) => value(d).n));
  return (
    <figure className="rounded-lg border border-border bg-surface p-3">
      <figcaption className="text-xs text-muted">{label}</figcaption>
      <div role="img" aria-label={label} className="mt-2 flex h-24 items-end gap-px">
        {days.map((d) => {
          const v = value(d);
          return (
            <div key={d.day} title={`${d.day}: ${v.text}`} className="flex h-full flex-1 items-end">
              <div className="w-full rounded-t-sm bg-accent" style={{ height: `${(v.n / max) * 100}%`, minHeight: v.n ? 2 : 0 }} />
            </div>
          );
        })}
      </div>
      <div className="mt-1 flex justify-between text-xs text-muted tabular-nums">
        <span>{days[0]?.day}</span>
        <span>{days.at(-1)?.day}</span>
      </div>
    </figure>
  );
}

export default function MeDashboardPage() {
  const q = useOverview();
  const programs = usePrograms().data;
  // Metrics carry no currency, so summing across programs is only meaningful when they all share one.
  const currencies = new Set(programs?.map((p) => p.currency));
  const currency = currencies.size === 1 ? [...currencies][0] : null;
  const mixed = currencies.size > 1;
  const money = (cents: number) => formatCents(cents, currency ?? "");

  return (
    <section aria-labelledby="page-title" className="grid gap-3">
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          Dashboard
        </h1>
        <p className="text-xs text-muted">
          {q.data ? `Your performance, ${q.data.summary.from} to ${q.data.summary.to}.` : "Your performance over the last 30 days."}
        </p>
      </header>

      {q.isError && !q.data ? (
        <div className="rounded-lg border border-border bg-surface"><ErrorState error={q.error} what="your dashboard" onRetry={q.refetch} /></div>
      ) : !q.data ? (
        <div className="grid gap-3" aria-busy>
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
            {[0, 1, 2, 3].map((i) => (
              <Skeleton key={i} className="h-20" />
            ))}
          </div>
          <div className="grid gap-3 lg:grid-cols-2">
            <Skeleton className="h-36" />
            <Skeleton className="h-36" />
          </div>
        </div>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
            <StatCard label="Clicks" value={q.data.summary.clicks} icon={MousePointerClick} />
            <StatCard label="Conversions" value={q.data.summary.conversions} icon={ReceiptText} />
            <StatCard label="Conversion rate" value={formatBps(q.data.summary.conversion_rate_bps)} icon={Percent} />
            <StatCard
              label="Commission"
              value={mixed ? "Mixed currencies" : money(q.data.summary.commission_cents)}
              hint={mixed ? "Your programs pay in different currencies, so there is no single total." : undefined}
              icon={Banknote}
              accent
            />
          </div>
          {q.data.summary.clicks === 0 && q.data.summary.conversions === 0 && (
            <p className="rounded-lg border border-dashed border-border px-4 py-3 text-xs text-muted">
              No activity in this period yet. Create a link under Links and share it to start earning.
            </p>
          )}
          <div className="grid gap-3 lg:grid-cols-2">
            <DayBars label="Clicks per day" days={q.data.days} value={(d) => ({ n: d.clicks, text: String(d.clicks) })} />
            {!mixed && (
              <DayBars
                label="Commission per day"
                days={q.data.days}
                value={(d) => ({ n: d.commission_cents, text: money(d.commission_cents) })}
              />
            )}
          </div>
        </>
      )}
    </section>
  );
}
