"use client";

import { useMemo, useState } from "react";
import { MetricCards } from "@/components/metric-cards";
import { Button } from "@/components/ui/button";
import { Label, Select } from "@/components/ui/input";
import { usePrograms } from "@/lib/admin";
import { currenciesOf, currencyFor, lastDays, useSummary, useSummaryAll } from "@/lib/analytics";
import { errorMessage } from "@/lib/api";
import { formatCents } from "@/lib/admin";

const all = { program_id: "", affiliate_id: "" };

export default function DashboardPage() {
  const range = useMemo(() => lastDays(30), []);
  const summary = useSummary(range, all, null);
  const programs = usePrograms();
  // Shown only when programs use more than one currency: the common case gets no extra UI.
  const currencies = currenciesOf(programs.data);
  const multi = currencies.length > 1;
  const [display, setDisplay] = useState("");
  const chosen = multi ? display || (currencies.includes("USD") ? "USD" : (currencies[0] ?? "")) : null;
  const converted = useSummaryAll(range, chosen);

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
      {multi && chosen && (
        <div className="grid gap-2 rounded-lg border border-border bg-surface p-3 text-xs">
          <div className="flex flex-wrap items-center gap-2">
            <Label htmlFor="display-currency">Display currency</Label>
            <Select id="display-currency" value={chosen} onChange={(e) => setDisplay(e.target.value)}>
              {currencies.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </Select>
            <span className="text-muted">Converted day by day at the latest cached rate.</span>
          </div>
          {converted.isError ? (
            <p role="alert" className="text-danger">
              Could not convert: {errorMessage(converted.error)}
            </p>
          ) : (
            <p>
              Revenue {converted.data ? formatCents(converted.data.revenue_cents, chosen) : "—"} · Commission{" "}
              {converted.data ? formatCents(converted.data.commission_cents, chosen) : "—"}
            </p>
          )}
          {converted.data && converted.data.unconverted.length > 0 && (
            <p role="status" className="text-warning">
              Not included (no exchange rate yet):{" "}
              {converted.data.unconverted
                .map((u) => `${formatCents(u.revenue_cents, u.currency)} revenue`)
                .join(", ")}
            </p>
          )}
        </div>
      )}
      <MetricCards summary={summary.data} currency={currencyFor(programs.data, "")} suffix="30d" />
    </section>
  );
}
