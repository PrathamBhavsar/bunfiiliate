"use client";

import { Plus } from "lucide-react";
import { programBody, programFields, ProgramDetail } from "@/components/program-detail";
import { FormDialog } from "@/components/form-dialog";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { Button } from "@/components/ui/button";
import { listParams, ok, refresh, required, toPage, type Program } from "@/lib/admin";
import { api } from "@/lib/api";
import type { ListFn } from "@/lib/use-resource";

const list: ListFn<Program> = async ({ page, pageSize }, signal) =>
  toPage(await ok(api.GET("/api/v1/programs", { signal, params: listParams({}, { page, limit: pageSize }) })));

const config: ResourceConfig<Program> = {
  title: "Programs",
  description: "Commission programs: settings, keys, sites, plans and enrolled affiliates.",
  list,
  get: (id, signal) => ok(api.GET("/api/v1/programs/{id}", { signal, params: { path: { id } } })),
  columns: [
    { id: "name", header: "Name", cell: (p) => <span className="font-medium">{p.name}</span> },
    { id: "currency", header: "Currency", cell: (p) => p.currency },
    { id: "attribution", header: "Attribution", cell: (p) => p.attribution_model.replace("_", " ") },
    { id: "cookie", header: "Cookie days", align: "right", cell: (p) => p.cookie_days },
    { id: "hold", header: "Hold days", align: "right", cell: (p) => p.hold_days },
    { id: "auto", header: "Auto-approve", cell: (p) => (p.auto_approve ? "Yes" : "No") },
  ],
  actions: (
    <FormDialog
      trigger={
        <Button variant="primary">
          <Plus /> New program
        </Button>
      }
      title="New program"
      fields={programFields({ create: true })}
      initial={{ attribution_model: "last_touch", cookie_days: "30", hold_days: "0", auto_approve: "false" }}
      submitLabel="Create"
      onSubmit={async (v) => {
        const currency = required(v.currency ?? "", "Currency").toUpperCase();
        if (!/^[A-Z]{3}$/.test(currency)) throw new Error("Currency must be a 3-letter code, e.g. USD");
        await ok(api.POST("/api/v1/programs", { body: { ...programBody(v), currency } }));
        await refresh();
      }}
    />
  ),
  detailTitle: (p) => p.name,
  detail: (p) => <ProgramDetail program={p} />,
  empty: { title: "No programs yet", description: "Create a program to start tracking referrals and paying commissions." },
};

export default function Page() {
  return <ResourceView config={config} />;
}
