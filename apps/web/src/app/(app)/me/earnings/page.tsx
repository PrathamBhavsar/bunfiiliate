"use client";

import { useMemo } from "react";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { formatCents, ok, toPage, usePrograms } from "@/lib/admin";
import { api } from "@/lib/api";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Row = components["schemas"]["Earning"];
type Status = components["schemas"]["ConversionStatus"];

const STATUSES: Status[] = ["pending", "approved", "paid", "rejected", "reversed"];

const day = (iso: string) => iso.slice(0, 10);

const list: ListFn<Row> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/me/earnings", {
        signal,
        params: {
          query: {
            page,
            limit: pageSize,
            program_id: filters.program_id || undefined,
            status: (filters.status || undefined) as Status | undefined,
          },
        },
      }),
    ),
  );

export default function Page() {
  const programs = usePrograms().data;

  const config = useMemo<ResourceConfig<Row>>(() => {
    const programName = (id: string) => programs?.find((p) => p.id === id)?.name ?? id;
    return {
      title: "Earnings",
      description: "Commission on each conversion you referred.",
      list,
      filters: [
        { id: "program_id", label: "Program", options: (programs ?? []).map((p) => ({ value: p.id, label: p.name })) },
        { id: "status", label: "Status", options: STATUSES.map((s) => ({ value: s, label: s })) },
      ],
      columns: [
        { id: "occurred", header: "Date", cell: (e) => day(e.occurred_at) },
        { id: "program", header: "Program", cell: (e) => programName(e.program_id) },
        { id: "amount", header: "Sale", align: "right", className: "tabular-nums", cell: (e) => formatCents(e.amount_cents, e.currency) },
        { id: "commission", header: "Commission", align: "right", className: "tabular-nums", cell: (e) => formatCents(e.commission_cents, e.currency) },
        { id: "status", header: "Status", cell: (e) => <StatusBadge status={e.status} /> },
        { id: "approved", header: "Approved", cell: (e) => (e.approved_at ? day(e.approved_at) : "—") },
      ],
      empty: { title: "No earnings yet", description: "Conversions from your links show up here once they are recorded." },
    };
  }, [programs]);

  return <ResourceView config={config} />;
}
