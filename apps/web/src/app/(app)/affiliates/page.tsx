"use client";

import { Facts, Section } from "@/components/admin-ui";
import { ResetTokenDialog } from "@/components/reset-token-dialog";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { FormDialog } from "@/components/form-dialog";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { type Affiliate, ok, refresh, toPage, wholeNumber } from "@/lib/admin";
import { api } from "@/lib/api";
import type { ListFn } from "@/lib/use-resource";

const list: ListFn<Affiliate> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/affiliates", {
        signal,
        params: { query: { page, limit: pageSize, status: filters.status || undefined } },
      }),
    ),
  );

type Action = "approve" | "suspend" | "reactivate";

const ACTIONS: Record<Action, { label: string; description: string; destructive?: boolean }> = {
  approve: { label: "Approve", description: "Activates the affiliate. They can then be enrolled in programs." },
  suspend: {
    label: "Suspend",
    description: "Blocks the affiliate and deletes all of their referral codes. Reactivating does not restore the codes.",
    destructive: true,
  },
  reactivate: { label: "Reactivate", description: "Activates the affiliate again. Their old referral codes are gone." },
};

const ALLOWED: Record<string, Action[]> = {
  pending: ["approve", "suspend"],
  active: ["suspend"],
  suspended: ["reactivate"],
};

function AffiliateDetail({ affiliate: a }: { affiliate: Affiliate }) {
  return (
    <div className="grid gap-4">
      <Section title="Profile">
        <Facts
          rows={[
            ["Email", a.email],
            ["Status", <StatusBadge key="s" status={a.status} />],
            ["Payout email", a.payout_email ?? <span className="text-muted">Not set</span>],
            ["Min payout", `${a.payout_min_cents} cents`],
          ]}
        />
      </Section>
      <Section title="Status">
        <div className="flex flex-wrap gap-2">
          {(ALLOWED[a.status] ?? []).map((action) => (
            <ConfirmDialog
              key={action}
              trigger={
                <Button size="sm" variant={ACTIONS[action].destructive ? "secondary" : "primary"}>
                  {ACTIONS[action].label}
                </Button>
              }
              title={`${ACTIONS[action].label} ${a.email}?`}
              description={ACTIONS[action].description}
              confirmLabel={ACTIONS[action].label}
              destructive={ACTIONS[action].destructive}
              onConfirm={async () => {
                await ok(api.POST(`/api/v1/affiliates/{id}/${action}`, { params: { path: { id: a.id } } }));
                await refresh();
              }}
            />
          ))}
        </div>
      </Section>
      <Section title="Account recovery">
        <div className="flex flex-wrap items-center gap-2">
          <ResetTokenDialog userId={a.user_id} email={a.email} />
        </div>
        <p className="text-xs text-muted">Use when the affiliate lost access. The token works once for an hour.</p>
      </Section>
      <Section
        title="Payout settings"
        action={
          <FormDialog
            trigger={<Button size="sm">Edit</Button>}
            title="Payout settings"
            fields={[
              { name: "payout_email", label: "Payout email", type: "email" },
              { name: "payout_min_cents", label: "Minimum payout (cents)", type: "number", required: true, hint: "Integer cents. 0 or more." },
            ]}
            initial={{ payout_email: a.payout_email ?? "", payout_min_cents: String(a.payout_min_cents) }}
            onSubmit={async (v) => {
              const email = (v.payout_email ?? "").trim();
              if (email && !email.includes("@")) throw new Error("Payout email is not a valid address");
              await ok(
                api.PATCH("/api/v1/affiliates/{id}", {
                  params: { path: { id: a.id } },
                  body: {
                    payout_email: email || null,
                    payout_min_cents: wholeNumber(v.payout_min_cents ?? "", "Minimum payout", 0, Number.MAX_SAFE_INTEGER),
                  },
                }),
              );
              await refresh();
            }}
          />
        }
      >
        <p className="text-xs text-muted">Where and from what balance this affiliate gets paid.</p>
      </Section>
    </div>
  );
}

const config: ResourceConfig<Affiliate> = {
  title: "Affiliates",
  description: "Approve applicants, suspend or reactivate partners, and set payout details.",
  list,
  get: (id, signal) => ok(api.GET("/api/v1/affiliates/{id}", { signal, params: { path: { id } } })),
  filters: [
    {
      id: "status",
      label: "Status",
      options: [
        { value: "pending", label: "Pending" },
        { value: "active", label: "Active" },
        { value: "suspended", label: "Suspended" },
      ],
    },
  ],
  columns: [
    { id: "email", header: "Email", cell: (a) => <span className="font-medium">{a.email}</span> },
    { id: "status", header: "Status", cell: (a) => <StatusBadge status={a.status} /> },
    { id: "payout", header: "Payout email", cell: (a) => a.payout_email ?? "—" },
    { id: "min", header: "Min payout (cents)", align: "right", cell: (a) => a.payout_min_cents },
  ],
  detailTitle: (a) => a.email,
  detail: (a) => <AffiliateDetail affiliate={a} />,
  empty: { title: "No affiliates", description: "Affiliates appear here after they register." },
};

export default function Page() {
  return <ResourceView config={config} />;
}
