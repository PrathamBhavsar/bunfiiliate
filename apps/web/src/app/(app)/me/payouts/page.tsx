"use client";

import { useMemo } from "react";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { formatCents, ok, toPage, usePrograms } from "@/lib/admin";
import { api } from "@/lib/api";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Row = components["schemas"]["MyPayout"];

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
      columns: [
        { id: "program", header: "Program", cell: (p) => programName(p.program_id) },
        { id: "amount", header: "Amount", align: "right", className: "tabular-nums", cell: (p) => formatCents(p.amount_cents, p.currency) },
        { id: "status", header: "Status", cell: (p) => <StatusBadge status={p.status} /> },
        { id: "paid", header: "Paid", cell: (p) => (p.paid_at ? p.paid_at.slice(0, 10) : "—") },
      ],
      empty: { title: "No payouts yet", description: "Payouts appear here once your balance reaches your minimum and a batch is approved." },
    };
  }, [programs]);

  return <ResourceView config={config} />;
}
