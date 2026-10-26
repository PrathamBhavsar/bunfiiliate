"use client";

import { useMemo } from "react";
import { MetricCards } from "@/components/metric-cards";
import { Button } from "@/components/ui/button";
import { usePrograms } from "@/lib/admin";
import { currencyFor, lastDays, useSummary } from "@/lib/analytics";
import { errorMessage } from "@/lib/api";

const all = { program_id: "", affiliate_id: "" };

export default function DashboardPage() {
  const range = useMemo(() => lastDays(30), []);
  const summary = useSummary(range, all, null);
  const programs = usePrograms();

  return (
    <section aria-labelledby="page-title" className="grid gap-3">
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          Dashboard
        </h1>
        <p className="text-xs text-muted">Last 30 days, all programs. Charts and breakdowns are under Analytics.</p>
      </header>
      {summary.isError && (
        <div role="alert" className="flex items-center gap-2 text-xs text-danger">
          <span>Could not load figures: {errorMessage(summary.error)}</span>
          <Button size="sm" onClick={() => summary.refetch()}>
            Retry
          </Button>
        </div>
      )}
      <MetricCards summary={summary.data} currency={currencyFor(programs.data, "")} suffix="30d" />
    </section>
  );
}
