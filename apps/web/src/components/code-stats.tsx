"use client";

import { ChevronRight, TriangleAlert } from "lucide-react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Suspense, useMemo, useState } from "react";
import { formatCount } from "@/components/metric-cards";
import { LineChart } from "@/components/charts";
import { StatCard } from "@/components/stat-card";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, Td, Th } from "@/components/ui/table";
import { formatCents } from "@/lib/admin";
import { CODE_NOT_FOUND, lastDays, useCodeStats, type CodeStatsScope } from "@/lib/analytics";
import { errorMessage } from "@/lib/api";

const RANGES = [7, 30, 90];

function Crumbs({ scope, code }: { scope: CodeStatsScope; code: string }) {
  const list = scope === "admin" ? { href: "/referral-codes", label: "Referral codes" } : { href: "/me/links", label: "Links" };
  return (
    <nav aria-label="Breadcrumb" className="flex items-center gap-1 text-xs text-muted">
      <Link href={list.href} className="hover:underline">
        {list.label}
      </Link>
      <ChevronRight aria-hidden className="size-3" />
      <span aria-current="page">{code || "Stats"}</span>
    </nav>
  );
}

function Inner({ scope }: { scope: CodeStatsScope }) {
  const code = useSearchParams().get("code") ?? "";
  const [days, setDays] = useState(30);
  const range = useMemo(() => lastDays(days), [days]);
  const stats = useCodeStats(scope, code, range);
  const d = stats.data;
  const money = (c: number) => formatCents(c, d?.currency ?? "");
  const notFound = stats.error instanceof Error && stats.error.message === CODE_NOT_FOUND;
  const points = (pick: (p: NonNullable<typeof d>["series"][number]) => number) =>
    d?.series.map((p) => ({ day: p.day, value: pick(p) })) ?? [];

  return (
    <section aria-labelledby="page-title" className="grid gap-3">
      <Crumbs scope={scope} code={code} />
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          {code ? <>Code <code>{code}</code></> : "Code stats"}
        </h1>
        <p className="text-xs text-muted">Clicks and conversions per UTC day. Rejected and reversed conversions are not counted.</p>
      </header>

      {!code || notFound ? (
        <div role="status" data-testid="code-not-found" className="grid justify-items-center gap-2 rounded-lg border border-border bg-surface px-4 py-12 text-center">
          <p className="font-medium">Code not found</p>
          <p className="text-xs text-muted">It may have been deactivated, or it is not yours to view.</p>
        </div>
      ) : stats.error ? (
        <div role="alert" className="grid justify-items-center gap-2 rounded-lg border border-border bg-surface px-4 py-12 text-center">
          <TriangleAlert aria-hidden className="size-5 text-danger" />
          <p className="font-medium">Could not load stats</p>
          <p className="text-xs text-muted">{errorMessage(stats.error)}</p>
          <Button size="sm" onClick={() => void stats.refetch()}>
            Retry
          </Button>
        </div>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <Select aria-label="Range" className="w-auto" value={days} onChange={(e) => setDays(Number(e.target.value))}>
              {RANGES.map((n) => (
                <option key={n} value={n}>
                  Last {n} days
                </option>
              ))}
            </Select>
          </div>

          {!d ? (
            <div className="grid gap-3 lg:grid-cols-2" aria-busy>
              <Skeleton className="h-52" />
              <Skeleton className="h-52" />
            </div>
          ) : (
            <>
              <div className="grid grid-cols-2 gap-3 lg:grid-cols-5" data-testid="code-kpis">
                <StatCard label="Clicks" value={formatCount(d.totals.clicks)} accent />
                <StatCard label="Flagged clicks" value={formatCount(d.totals.flagged_clicks)} />
                <StatCard label="Conversions" value={formatCount(d.totals.conversions)} />
                <StatCard label="Revenue" value={money(d.totals.revenue_cents)} />
                <StatCard label="Commission" value={money(d.totals.commission_cents)} />
              </div>
              <div className="grid gap-3 lg:grid-cols-2">
                <LineChart title="Clicks" color="var(--series-1)" points={points((p) => p.clicks)} format={formatCount} />
                <LineChart title="Conversions" color="var(--series-2)" points={points((p) => p.conversions)} format={formatCount} />
              </div>
              <div className="grid gap-3 lg:grid-cols-2">
                <div className="rounded-lg border border-border bg-surface">
                  <h2 className="p-3 text-xs font-semibold">Top countries</h2>
                  {d.top_countries.length === 0 ? (
                    <p className="border-t border-border p-3 text-xs text-muted">No clicks with a known country.</p>
                  ) : (
                    <Table aria-label="Top countries">
                      <thead>
                        <tr>
                          <Th>Country</Th>
                          <Th className="text-right">Clicks</Th>
                        </tr>
                      </thead>
                      <tbody>
                        {d.top_countries.map((c) => (
                          <tr key={c.country}>
                            <Td>{c.country}</Td>
                            <Td className="text-right tabular-nums">{formatCount(c.clicks)}</Td>
                          </tr>
                        ))}
                      </tbody>
                    </Table>
                  )}
                </div>
                <div className="rounded-lg border border-border bg-surface">
                  <h2 className="p-3 text-xs font-semibold">Top referrers</h2>
                  {d.top_referrers.length === 0 ? (
                    <p className="border-t border-border p-3 text-xs text-muted">No clicks with a referrer.</p>
                  ) : (
                    <Table aria-label="Top referrers">
                      <thead>
                        <tr>
                          <Th>Referrer</Th>
                          <Th className="text-right">Clicks</Th>
                        </tr>
                      </thead>
                      <tbody>
                        {d.top_referrers.map((r) => (
                          <tr key={r.host}>
                            <Td>{r.host}</Td>
                            <Td className="text-right tabular-nums">{formatCount(r.clicks)}</Td>
                          </tr>
                        ))}
                      </tbody>
                    </Table>
                  )}
                </div>
              </div>
            </>
          )}
        </>
      )}
    </section>
  );
}

/** Stats page body for `?code=`. `useSearchParams` needs the Suspense boundary in a static export. */
export function CodeStatsPage({ scope }: { scope: CodeStatsScope }) {
  return (
    <Suspense fallback={<Skeleton className="h-52" />}>
      <Inner scope={scope} />
    </Suspense>
  );
}
