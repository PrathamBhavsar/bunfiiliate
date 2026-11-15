"use client";

import { useMemo } from "react";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { Plus } from "lucide-react";
import { FormDialog } from "@/components/form-dialog";
import { Button } from "@/components/ui/button";
import { formatCents, formatTime, ok, parseCents, refresh, required, toPage, usePrograms } from "@/lib/admin";
import { api } from "@/lib/api";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Row = components["schemas"]["MyPayout"];
type Request = components["schemas"]["RequestView"];

const requests: ListFn<Request> = async ({ page, pageSize }, signal) =>
  toPage(await ok(api.GET("/api/v1/me/payout-requests", { signal, params: { query: { page, limit: pageSize } } })));

const list: ListFn<Row> = async ({ page, pageSize }, signal) =>
  toPage(await ok(api.GET("/api/v1/me/payouts", { signal, params: { query: { page, limit: pageSize } } })));

export default function Page() {
  const programs = usePrograms().data;

  const config = useMemo<ResourceConfig<Row>>(() => {
    const programName = (id: string) => programs?.find((p) => p.id === id)?.name ?? id;
    return {
      title: "Payouts",
      description: "Approved payouts are owed to you; paid ones have been sent.",
      list,
      actions: (
        <FormDialog
          trigger={
            <Button variant="primary">
              <Plus /> Request payout
            </Button>
          }
          title="Request a payout"
          description="Asks the program owner to pay out part or all of your approved balance. The amount is checked against what you are owed when you submit."
          fields={[
            {
              name: "program_id",
              label: "Program",
              type: "select",
              required: true,
              options: [{ value: "", label: "Choose…" }, ...(programs ?? []).map((p) => ({ value: p.id, label: p.name }))],
            },
            { name: "amount", label: "Amount", required: true, placeholder: "25.00", hint: "In the program's currency, e.g. 25 or 25.50." },
          ]}
          submitLabel="Send request"
          onSubmit={async (v) => {
            const program = programs?.find((p) => p.id === required(v.program_id ?? "", "Program"));
            if (!program) throw new Error("Program is required");
            await ok(
              api.POST("/api/v1/me/payout-requests", {
                body: { program_id: program.id, requested_cents: parseCents(v.amount ?? "", "Amount"), currency: program.currency },
              }),
            );
            await refresh();
          }}
        />
      ),
      columns: [
        { id: "program", header: "Program", cell: (p) => programName(p.program_id) },
        { id: "amount", header: "Amount", align: "right", className: "tabular-nums", cell: (p) => formatCents(p.amount_cents, p.currency) },
        { id: "status", header: "Status", cell: (p) => <StatusBadge status={p.status} /> },
        { id: "paid", header: "Paid", cell: (p) => (p.paid_at ? p.paid_at.slice(0, 10) : "—") },
      ],
      empty: { title: "No payouts yet", description: "Payouts appear here once your balance reaches your minimum and a batch is approved." },
    };
  }, [programs]);

  const requestConfig = useMemo<ResourceConfig<Request>>(() => {
    const programName = (id: string) => programs?.find((p) => p.id === id)?.name ?? id;
    return {
      title: "Payout requests",
      description: "Requests you sent. One can be pending per program at a time.",
      list: requests,
      columns: [
        { id: "created", header: "Requested", cell: (r) => formatTime(r.created_at) },
        { id: "program", header: "Program", cell: (r) => programName(r.program_id) },
        { id: "amount", header: "Amount", align: "right", className: "tabular-nums", cell: (r) => formatCents(r.requested_cents, r.currency) },
        { id: "status", header: "Status", cell: (r) => <StatusBadge status={r.status} /> },
        { id: "reason", header: "Reason", className: "max-w-64 truncate", cell: (r) => r.rejection_reason ?? "—" },
      ],
      empty: { title: "No requests yet", description: "Use Request payout above when you have an approved balance." },
    };
  }, [programs]);

  return (
    <div className="grid gap-4">
      <ResourceView config={config} />
      <ResourceView config={requestConfig} headingLevel={2} />
    </div>
  );
}
