"use client";

import { useQuery } from "@tanstack/react-query";
import { Pencil, TriangleAlert } from "lucide-react";
import { Facts } from "@/components/admin-ui";
import { FormDialog } from "@/components/form-dialog";
import { SecuritySection } from "@/components/security-section";
import { StatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { ok, refresh, wholeNumber } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";

// Not under "me": refresh() leaves the ["me"] session query alone by key prefix.
const profileKey = ["portal", "profile"] as const;

export default function ProfilePage() {
  const q = useQuery({
    queryKey: profileKey,
    queryFn: ({ signal }) => ok(api.GET("/api/v1/me/affiliate", { signal })),
  });
  const a = q.data;

  return (
    <section aria-labelledby="page-title" className="grid max-w-xl gap-3">
      <header className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <h1 id="page-title" className="text-base font-semibold">
            Profile
          </h1>
          <p className="text-xs text-muted">Where and when we pay you.</p>
        </div>
        {a && (
          <FormDialog
            trigger={
              <Button variant="primary">
                <Pencil /> Edit
              </Button>
            }
            title="Payout settings"
            fields={[
              { name: "payout_email", label: "Payout email", type: "email", placeholder: a.email, hint: "Where payouts are sent. Defaults to your account email." },
              { name: "payout_min_cents", label: "Minimum payout (cents)", type: "number", required: true, hint: "A payout is created once your balance reaches this amount. 5000 = 50.00." },
            ]}
            initial={{ payout_email: a.payout_email ?? "", payout_min_cents: String(a.payout_min_cents) }}
            onSubmit={async (v) => {
              const email = (v.payout_email ?? "").trim();
              await ok(
                api.PATCH("/api/v1/me/affiliate", {
                  body: { payout_email: email || null, payout_min_cents: wholeNumber(v.payout_min_cents ?? "", "Minimum payout", 0) },
                }),
              );
              await refresh();
            }}
          />
        )}
      </header>

      <div className="rounded-lg border border-border bg-surface p-4">
        {q.isError && !a ? (
          <div role="alert" className="grid justify-items-center gap-2 py-8 text-center">
            <TriangleAlert aria-hidden className="size-5 text-danger" />
            <p className="font-medium">Could not load your profile</p>
            <p className="text-xs text-muted">{errorMessage(q.error)}</p>
            <Button size="sm" onClick={() => q.refetch()}>
              Retry
            </Button>
          </div>
        ) : !a ? (
          <Skeleton className="h-24" aria-busy />
        ) : (
          <Facts
            rows={[
              ["Account email", a.email],
              ["Status", <StatusBadge key="s" status={a.status} />],
              ["Payout email", a.payout_email ?? `${a.email} (account email)`],
              ["Minimum payout", `${a.payout_min_cents} cents`],
            ]}
          />
        )}
      </div>
      <SecuritySection />
    </section>
  );
}
