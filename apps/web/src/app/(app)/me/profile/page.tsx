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
const taxKey = ["portal", "tax-form"] as const;

/** W-8/W-9 capture. Only the last 4 digits of a tax id are ever sent or stored. */
function TaxFormSection() {
  const q = useQuery({
    queryKey: taxKey,
    queryFn: ({ signal }) => ok(api.GET("/api/v1/me/tax-form", { signal })),
  });
  const t = q.data;
  return (
    <div className="grid gap-2 rounded-lg border border-border bg-surface p-4">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-semibold">Tax form</h2>
        {t && (
          <FormDialog
            trigger={
              <Button variant="primary">
                <Pencil /> {t.kind === "none" ? "Add" : "Edit"}
              </Button>
            }
            title="Tax form"
            description="Enter the last 4 digits of your tax id only. Never type the full number."
            fields={[
              {
                name: "kind",
                label: "Form",
                type: "select",
                required: true,
                options: [
                  { value: "w9", label: "W-9 (US person)" },
                  { value: "w8", label: "W-8 (non-US person)" },
                ],
              },
              { name: "legal_name", label: "Legal name", required: true },
              { name: "country", label: "Country (2 letters)", required: true, placeholder: "US" },
              { name: "tax_id_last4", label: "Tax id, last 4 digits", placeholder: "1234" },
            ]}
            initial={{ kind: t.kind === "w8" ? "w8" : "w9", legal_name: t.legal_name ?? "", country: t.country ?? "", tax_id_last4: t.tax_id_last4 ?? "" }}
            onSubmit={async (v) => {
              const last4 = (v.tax_id_last4 ?? "").trim();
              await ok(
                api.PUT("/api/v1/me/tax-form", {
                  body: { kind: v.kind ?? "w9", legal_name: v.legal_name ?? "", country: v.country ?? "", tax_id_last4: last4 || null },
                }),
              );
              await refresh();
            }}
          />
        )}
      </div>
      {q.isError && !t ? (
        <p role="alert" className="text-xs text-danger">
          Could not load your tax form: {errorMessage(q.error)}
        </p>
      ) : !t ? (
        <Skeleton className="h-16" aria-busy />
      ) : t.kind === "none" ? (
        <p className="text-xs text-muted">No tax form on file. Invoices use your payout email until you add one.</p>
      ) : (
        <Facts
          rows={[
            ["Form", t.kind.toUpperCase()],
            ["Legal name", t.legal_name ?? "—"],
            ["Country", t.country ?? "—"],
            ["Tax id", t.tax_id_last4 ? `•••• ${t.tax_id_last4}` : "—"],
          ]}
        />
      )}
    </div>
  );
}

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
      <TaxFormSection />
      <SecuritySection />
    </section>
  );
}
