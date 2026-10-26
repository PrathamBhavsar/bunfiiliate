"use client";

import { useMemo } from "react";
import { Facts, Section } from "@/components/admin-ui";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import {
  type Conversion,
  type ConversionStatus,
  formatCents,
  formatTime,
  ok,
  refresh,
  toPage,
  useLookups,
} from "@/lib/admin";
import { api } from "@/lib/api";
import type { ListFn } from "@/lib/use-resource";

const STATUSES: ConversionStatus[] = ["pending", "approved", "paid", "rejected", "reversed"];

const list: ListFn<Conversion> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/conversions", {
        signal,
        params: {
          query: {
            page,
            limit: pageSize,
            program_id: filters.program_id || undefined,
            affiliate_id: filters.affiliate_id || undefined,
            status: (filters.status || undefined) as ConversionStatus | undefined,
            from: filters.from || undefined,
            to: filters.to || undefined,
          },
        },
      }),
    ),
  );

type Action = "approve" | "reject" | "reverse";

const ACTIONS: Record<Action, { label: string; destructive?: boolean; describe: (c: Conversion) => string }> = {
  approve: {
    label: "Approve",
    describe: (c) => `Moves ${formatCents(c.commission_cents, c.currency)} of commission from pending to payable.`,
  },
  reject: {
    label: "Reject",
    destructive: true,
    describe: (c) =>
      `Cancels ${formatCents(c.commission_cents, c.currency)} of pending commission. A rejected conversion cannot be approved later.`,
  },
  reverse: {
    label: "Reverse",
    destructive: true,
    describe: (c) =>
      `Claws back ${formatCents(c.commission_cents, c.currency)}. If it was already paid out, the amount is netted against the affiliate's next payout.`,
  },
};

const ALLOWED: Record<string, Action[] | undefined> = {
  pending: ["approve", "reject"],
  approved: ["reverse"],
  paid: ["reverse"],
};

const allowed = (status: string) => ALLOWED[status] ?? [];

export default function Page() {
  const lookups = useLookups();

  const config = useMemo<ResourceConfig<Conversion>>(
    () => {
      const { programOptions, affiliateOptions, programName, affiliateEmail } = lookups;
      return {
      title: "Conversions",
      description: "Sales credited to affiliates. Review pending ones, reverse refunded ones.",
      list,
      get: (id, signal) => ok(api.GET("/api/v1/conversions/{id}", { signal, params: { path: { id } } })),
      filters: [
        { id: "program_id", label: "Program", options: programOptions },
        { id: "affiliate_id", label: "Affiliate", options: affiliateOptions },
        { id: "status", label: "Status", options: STATUSES.map((s) => ({ value: s, label: s })) },
        { id: "from", label: "From", type: "date" },
        { id: "to", label: "To", type: "date" },
      ],
      columns: [
        { id: "occurred", header: "Occurred", cell: (c) => formatTime(c.occurred_at) },
        { id: "external", header: "Sale ID", cell: (c) => <code>{c.external_id}</code> },
        { id: "program", header: "Program", cell: (c) => programName(c.program_id) },
        { id: "affiliate", header: "Affiliate", cell: (c) => affiliateEmail(c.affiliate_id) },
        { id: "amount", header: "Sale", align: "right", cell: (c) => formatCents(c.amount_cents, c.currency) },
        { id: "commission", header: "Commission", align: "right", cell: (c) => formatCents(c.commission_cents, c.currency) },
        { id: "status", header: "Status", cell: (c) => <StatusBadge status={c.status} /> },
      ],
      detailTitle: (c) => `Conversion ${c.external_id}`,
      detail: (c) => (
        <div className="grid gap-4">
          <Section title="Sale">
            <Facts
              rows={[
                ["Status", <StatusBadge key="s" status={c.status} />],
                ["Sale ID", <code key="e">{c.external_id}</code>],
                ["Program", programName(c.program_id)],
                ["Affiliate", affiliateEmail(c.affiliate_id)],
                ["Customer", <code key="c">{c.customer_id}</code>],
                ["Sale amount", formatCents(c.amount_cents, c.currency)],
                ["Commission", formatCents(c.commission_cents, c.currency)],
                ["Occurred", formatTime(c.occurred_at)],
                ["Approved", c.approved_at ? formatTime(c.approved_at) : "—"],
                ["Reversed", c.reversed_at ? formatTime(c.reversed_at) : "—"],
                ["Payout", c.payout_id ? <code key="p">{c.payout_id}</code> : "—"],
              ]}
            />
          </Section>
          <Section title="Review">
            {allowed(c.status).length === 0 ? (
              <p className="text-xs text-muted">A {c.status} conversion cannot change status.</p>
            ) : (
              <div className="flex flex-wrap gap-2">
                {allowed(c.status).map((action) => (
                  <ConfirmDialog
                    key={action}
                    trigger={
                      <Button size="sm" variant={action === "approve" ? "primary" : "secondary"}>
                        {ACTIONS[action].label}
                      </Button>
                    }
                    title={`${ACTIONS[action].label} conversion ${c.external_id}?`}
                    description={ACTIONS[action].describe(c)}
                    confirmLabel={ACTIONS[action].label}
                    destructive={ACTIONS[action].destructive}
                    onConfirm={async () => {
                      await ok(api.POST(`/api/v1/conversions/{id}/${action}`, { params: { path: { id: c.id } } }));
                      await refresh();
                    }}
                  />
                ))}
              </div>
            )}
          </Section>
        </div>
      ),
      empty: { title: "No conversions", description: "Conversions appear here once tracked sales come in." },
    };
    },
    [lookups],
  );

  return <ResourceView config={config} />;
}
