"use client";

import { useQuery } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";
import { useState } from "react";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { formatCents, ok, refresh } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";

// Not under "me": refresh() leaves the ["me"] session query alone by key prefix.
const subKey = ["billing", "subscription"] as const;
const plansKey = ["billing", "plans"] as const;

/** A usage bar against a limit. A null limit means unlimited, so there is no bar. */
function Usage({ label, used, limit }: { label: string; used: number; limit: number | null | undefined }) {
  if (limit == null) {
    return (
      <div data-testid={`usage-${label}`} className="text-xs">
        {label}: {used} (unlimited)
      </div>
    );
  }
  const pct = limit === 0 ? 100 : Math.min(100, Math.round((used / limit) * 100));
  return (
    <div data-testid={`usage-${label}`} className="grid gap-1 text-xs">
      <div className="flex justify-between">
        <span>{label}</span>
        <span>
          {used} / {limit}
        </span>
      </div>
      <div role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={limit} aria-valuenow={Math.min(used, limit)} className="h-1.5 overflow-hidden rounded-full bg-subtle">
        <div className={pct >= 100 ? "h-full bg-danger" : "h-full bg-accent"} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export default function Page() {
  const sub = useQuery({ queryKey: subKey, queryFn: ({ signal }) => ok(api.GET("/api/v1/billing/subscription", { signal })) });
  const plans = useQuery({ queryKey: plansKey, queryFn: ({ signal }) => ok(api.GET("/api/v1/billing/plans", { signal })) });
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  /** Throws on failure so the confirm dialog stays open and shows the error. */
  async function change(planKey: string) {
    setResult(null);
    const res = await ok(api.POST("/api/v1/billing/checkout", { body: { plan_key: planKey, provider: sub.data?.provider } }));
    if (res.url) {
      window.location.assign(res.url);
      return;
    }
    setResult({ ok: true, text: "Plan changed." });
    await refresh();
  }

  const failed = sub.isError || plans.isError;
  const current = sub.data?.plan;

  return (
    <section aria-labelledby="page-title" className="grid max-w-2xl gap-3">
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          Billing
        </h1>
        <p className="text-xs text-muted">Your plan, what it allows and how much of it you use.</p>
      </header>
      {failed ? (
        <div role="alert" className="grid justify-items-center gap-2 rounded-lg border border-border bg-surface px-4 py-12 text-center">
          <TriangleAlert aria-hidden className="size-5 text-danger" />
          <p className="font-medium">Could not load billing</p>
          <p className="text-xs text-muted">{errorMessage(sub.error ?? plans.error)}</p>
          <Button
            size="sm"
            onClick={() => {
              void sub.refetch();
              void plans.refetch();
            }}
          >
            Retry
          </Button>
        </div>
      ) : sub.isPending || plans.isPending || !current ? (
        <Skeleton className="h-64" />
      ) : (
        <>
          <div className="grid gap-3 rounded-lg border border-border bg-surface p-3">
            <div className="flex items-baseline justify-between gap-2">
              <h2 className="text-sm font-semibold" data-testid="current-plan">
                {current.name}
              </h2>
              <span className="text-xs text-muted">
                {sub.data.status} · {sub.data.provider}
              </span>
            </div>
            <Usage label="Programs" used={sub.data.usage.programs} limit={current.max_programs} />
            <Usage label="Affiliates" used={sub.data.usage.affiliates} limit={current.max_affiliates} />
            <Usage label="Clicks this month" used={sub.data.usage.monthly_clicks} limit={current.max_monthly_clicks} />
            {sub.data.current_period_end && <p className="text-xs text-muted">Current period ends {new Date(sub.data.current_period_end).toLocaleDateString()}.</p>}
          </div>
          <ul className="grid gap-2" aria-label="Available plans">
            {plans.data.map((p) => (
              <li key={p.key} className="flex items-center justify-between gap-2 rounded-lg border border-border bg-surface p-3">
                <div>
                  <p className="text-sm font-medium">{p.name}</p>
                  <p className="text-xs text-muted">{p.price_cents === 0 ? "Free" : `${formatCents(p.price_cents, p.currency)} / month`}</p>
                </div>
                {p.key === current.key ? (
                  <span className="text-xs text-muted">Current plan</span>
                ) : (
                  <ConfirmDialog
                    trigger={
                      <Button size="sm" variant="primary">
                        {p.price_cents < current.price_cents ? "Downgrade" : "Upgrade"} to {p.name}
                      </Button>
                    }
                    title={`Switch to ${p.name}?`}
                    description="With the mock provider the plan changes immediately; other providers send you to a hosted checkout."
                    confirmLabel="Switch plan"
                    onConfirm={() => change(p.key)}
                  />
                )}
              </li>
            ))}
          </ul>
        </>
      )}
      {result && (
        <p role={result.ok ? "status" : "alert"} className={result.ok ? "text-xs text-success" : "text-xs text-danger"}>
          {result.text}
        </p>
      )}
    </section>
  );
}
