"use client";

import { ErrorState } from "@/components/state";
import { useMemo, useState } from "react";
import { ScopeSelects } from "@/components/analytics-filters";
import { formatDay, LineChart } from "@/components/charts";
import { formatCount, MetricCards } from "@/components/metric-cards";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, Td, Th } from "@/components/ui/table";
import { formatBps, formatCents, usePrograms } from "@/lib/admin";
import { currencyFor, DIMS, lastDays, useSummary, useTimeseries, type Dim, type Scope } from "@/lib/analytics";

const RANGES = [7, 30, 90];
const BREAKDOWNS = DIMS.filter((d) => d.value !== "total");
const TOP = 10;

export default function Page() {
  const [days, setDays] = useState(30);
  const [scope, setScope] = useState<Scope>({ program_id: "", affiliate_id: "" });
  const [dim, setDim] = useState<Dim>("country");
  const [showAll, setShowAll] = useState(false);
  const range = useMemo(() => lastDays(days), [days]);
  const summary = useSummary(range, scope, dim);
  const series = useTimeseries(range, scope);
  const currency = currencyFor(usePrograms().data, scope.program_id);

  const error = summary.error ?? series.error;
  const empty = summary.data && summary.data.clicks === 0 && summary.data.conversions === 0;
  const money = (c: number) => (currency === null ? "—" : formatCents(c, currency));
  const slices = summary.data?.breakdown ?? [];
  const maxClicks = Math.max(1, ...slices.map((s) => s.clicks));
  const visible = showAll ? slices : slices.slice(0, TOP);
  const points = (pick: (p: NonNullable<typeof series.data>[number]) => number) =>
    series.data?.map((p) => ({ day: p.day, value: pick(p) })) ?? [];

  return (
    <section aria-labelledby="page-title" className="grid gap-3">
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          Analytics
        </h1>
        <p className="text-xs text-muted">
          Clicks, conversions and commission per UTC day. Rejected and reversed conversions are not counted.
        </p>
      </header>

      <div className="flex flex-wrap items-center gap-2">
        <Select aria-label="Range" className="w-auto" value={days} onChange={(e) => setDays(Number(e.target.value))}>
          {RANGES.map((n) => (
            <option key={n} value={n}>
              Last {n} days
            </option>
          ))}
        </Select>
        <ScopeSelects scope={scope} onChange={setScope} />
      </div>

      {error ? (
        <div className="rounded-lg border border-border bg-surface"><ErrorState error={error} what="analytics" onRetry={() => { void summary.refetch(); void series.refetch(); }} /></div>
      ) : (
        <>
          <MetricCards summary={summary.data} currency={currency} suffix={`${days}d`} />

          {empty ? (
            <div className="rounded-lg border border-border bg-surface px-4 py-12 text-center">
              <p className="font-medium">No activity in this range</p>
              <p className="mx-auto max-w-sm text-xs text-muted">
                Clicks and conversions appear here after the next rollup tick (up to a minute). Try a longer range or another scope.
              </p>
            </div>
          ) : !series.data || !summary.data ? (
            <div className="grid gap-3 lg:grid-cols-3" aria-busy>
              <Skeleton className="h-52" />
              <Skeleton className="h-52" />
              <Skeleton className="h-52" />
            </div>
          ) : (
            <>
              <div className="grid gap-3 lg:grid-cols-3">
                <LineChart title="Clicks" color="var(--series-1)" points={points((p) => p.clicks)} format={formatCount} />
                <LineChart title="Conversions" color="var(--series-2)" points={points((p) => p.conversions)} format={formatCount} />
                {currency === null ? (
                  <div className="rounded-lg border border-border bg-surface p-3 text-xs">
                    <p className="font-semibold">Commission</p>
                    <p className="mt-1 text-muted">Programs use different currencies. Pick a program to chart commission.</p>
                  </div>
                ) : (
                  <LineChart
                    title="Commission"
                    color="var(--series-3)"
                    points={points((p) => p.commission_cents)}
                    format={(c) => formatCents(c, currency)}
                    formatAxis={(c) => formatCents(c, "")}
                  />
                )}
              </div>

              <div className="rounded-lg border border-border bg-surface">
                <div className="flex flex-wrap items-center justify-between gap-2 p-3">
                  <h2 className="text-xs font-semibold">Breakdown</h2>
                  <Select aria-label="Break down by" className="w-auto" value={dim} onChange={(e) => setDim(e.target.value as Dim)}>
                    {BREAKDOWNS.map((d) => (
                      <option key={d.value} value={d.value}>
                        By {d.label.toLowerCase()}
                      </option>
                    ))}
                  </Select>
                </div>
                {slices.length === 0 ? (
                  <p className="border-t border-border p-3 text-xs text-muted">Nothing to break down in this range.</p>
                ) : (
                  <>
                    <Table aria-label="Breakdown">
                      <thead>
                        <tr>
                          <Th>{BREAKDOWNS.find((d) => d.value === dim)?.label}</Th>
                          <Th className="text-right">Clicks</Th>
                          <Th className="text-right">Conversions</Th>
                          <Th className="text-right">Rate</Th>
                          <Th className="text-right">Revenue</Th>
                          <Th className="text-right">Commission</Th>
                        </tr>
                      </thead>
                      <tbody>
                        {visible.map((s) => (
                          <tr key={s.dim_value}>
                            <Td>{s.dim_value || <span className="text-muted">(unknown)</span>}</Td>
                            <Td className="text-right tabular-nums">
                              <span className="inline-flex items-center justify-end gap-2">
                                <span
                                  aria-hidden
                                  className="h-2 rounded-r-sm"
                                  style={{ width: `${Math.max(2, Math.round((s.clicks / maxClicks) * 80))}px`, background: "var(--series-1)" }}
                                />
                                {formatCount(s.clicks)}
                              </span>
                            </Td>
                            <Td className="text-right tabular-nums">{formatCount(s.conversions)}</Td>
                            <Td className="text-right tabular-nums">{formatBps(s.conversion_rate_bps)}</Td>
                            <Td className="text-right tabular-nums">{money(s.revenue_cents)}</Td>
                            <Td className="text-right tabular-nums">{money(s.commission_cents)}</Td>
                          </tr>
                        ))}
                      </tbody>
                    </Table>
                    {slices.length > TOP && (
                      <div className="border-t border-border p-2">
                        <Button size="sm" onClick={() => setShowAll((v) => !v)}>
                          {showAll ? `Show top ${TOP}` : `Show all ${slices.length}`}
                        </Button>
                      </div>
                    )}
                  </>
                )}
              </div>

              <details className="rounded-lg border border-border bg-surface">
                <summary className="cursor-pointer p-3 text-xs font-semibold">Daily data table</summary>
                <Table aria-label="Daily data">
                  <thead>
                    <tr>
                      <Th>Day</Th>
                      <Th className="text-right">Clicks</Th>
                      <Th className="text-right">Conversions</Th>
                      <Th className="text-right">Revenue</Th>
                      <Th className="text-right">Commission</Th>
                    </tr>
                  </thead>
                  <tbody>
                    {[...series.data].reverse().map((p) => (
                      <tr key={p.day}>
                        <Td>{formatDay(p.day)}</Td>
                        <Td className="text-right tabular-nums">{formatCount(p.clicks)}</Td>
                        <Td className="text-right tabular-nums">{formatCount(p.conversions)}</Td>
                        <Td className="text-right tabular-nums">{money(p.revenue_cents)}</Td>
                        <Td className="text-right tabular-nums">{money(p.commission_cents)}</Td>
                      </tr>
                    ))}
                  </tbody>
                </Table>
              </details>
            </>
          )}
        </>
      )}
    </section>
  );
}
