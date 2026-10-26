"use client";

import { Plus } from "lucide-react";
import { useMemo } from "react";
import { Facts, Section } from "@/components/admin-ui";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { FormDialog } from "@/components/form-dialog";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { Button } from "@/components/ui/button";
import {
  listParams,
  ok,
  refresh,
  required,
  toPage,
  useAffiliates,
  usePrograms,
  type ReferralCode,
} from "@/lib/admin";
import { api } from "@/lib/api";
import type { ListFn } from "@/lib/use-resource";

type Row = ReferralCode & { id: string };

const list: ListFn<Row> = async ({ page, pageSize, filters }, signal) => {
  const { items, total } = await ok(
    api.GET("/api/v1/referral-codes", {
      signal,
      params: listParams(
        {},
        { page, limit: pageSize, program_id: filters.program_id || null, affiliate_id: filters.affiliate_id || null },
      ),
    }),
  );
  return toPage({ items: items.map((c) => ({ ...c, id: c.code })), total });
};

export default function Page() {
  const programs = usePrograms().data;
  const affiliates = useAffiliates().data;

  const config = useMemo<ResourceConfig<Row>>(() => {
    const programName = (id: string) => programs?.find((p) => p.id === id)?.name ?? id;
    const affiliateEmail = (id: string) => affiliates?.find((a) => a.id === id)?.email ?? id;
    const active = (affiliates ?? []).filter((a) => a.status === "active");
    return {
      title: "Referral codes",
      description: "Tracking codes that attribute clicks to an affiliate in a program.",
      list,
      filters: [
        { id: "program_id", label: "Program", options: (programs ?? []).map((p) => ({ value: p.id, label: p.name })) },
        { id: "affiliate_id", label: "Affiliate", options: (affiliates ?? []).map((a) => ({ value: a.id, label: a.email })) },
      ],
      columns: [
        { id: "code", header: "Code", cell: (c) => <code className="font-medium">{c.code}</code> },
        { id: "affiliate", header: "Affiliate", cell: (c) => affiliateEmail(c.affiliate_id) },
        { id: "program", header: "Program", cell: (c) => programName(c.program_id) },
        { id: "landing", header: "Landing URL", className: "break-all", cell: (c) => c.landing_url },
      ],
      actions: (
        <FormDialog
          trigger={
            <Button variant="primary">
              <Plus /> New code
            </Button>
          }
          title="New referral code"
          description="The affiliate must be active and enrolled in the program."
          fields={[
            {
              name: "program_id",
              label: "Program",
              type: "select",
              required: true,
              options: [{ value: "", label: "Choose…" }, ...(programs ?? []).map((p) => ({ value: p.id, label: p.name }))],
            },
            {
              name: "affiliate_id",
              label: "Affiliate",
              type: "select",
              required: true,
              options: [{ value: "", label: "Choose…" }, ...active.map((a) => ({ value: a.id, label: a.email }))],
            },
            { name: "code", label: "Code", hint: "Optional. 3-32 characters of A-Z a-z 0-9 - _. Generated when empty." },
            { name: "landing_url", label: "Landing URL", type: "text", hint: "Optional. Must be on one of the program's sites. Defaults to its first site." },
          ]}
          submitLabel="Create"
          onSubmit={async (v) => {
            const code = (v.code ?? "").trim();
            if (code && !/^[A-Za-z0-9_-]{3,32}$/.test(code)) throw new Error("Code must be 3-32 characters of A-Z a-z 0-9 - _");
            const landing = (v.landing_url ?? "").trim();
            if (landing && !/^https?:\/\/[^\s/]+/i.test(landing)) throw new Error("Landing URL must start with http:// or https://");
            await ok(
              api.POST("/api/v1/referral-codes", {
                body: {
                  program_id: required(v.program_id ?? "", "Program"),
                  affiliate_id: required(v.affiliate_id ?? "", "Affiliate"),
                  code: code || undefined,
                  landing_url: landing || undefined,
                },
              }),
            );
            await refresh();
          }}
        />
      ),
      detailTitle: (c) => c.code,
      detail: (c) => (
        <div className="grid gap-4">
          <Section title="Code">
            <Facts
              rows={[
                ["Code", <code key="c">{c.code}</code>],
                ["Affiliate", affiliateEmail(c.affiliate_id)],
                ["Program", programName(c.program_id)],
                ["Landing URL", c.landing_url],
              ]}
            />
          </Section>
          <Section title="Danger zone">
            <ConfirmDialog
              trigger={<Button variant="danger" size="sm" className="justify-self-start">Delete code</Button>}
              title={`Delete ${c.code}?`}
              description="The link stops tracking right away. Clicks already recorded keep their attribution."
              confirmLabel="Delete"
              destructive
              onConfirm={async () => {
                await ok(api.DELETE("/api/v1/referral-codes/{code}", { params: { path: { code: c.code } } }));
                await refresh();
              }}
            />
          </Section>
        </div>
      ),
      empty: { title: "No referral codes", description: "Create a code for an enrolled affiliate to start tracking clicks." },
    };
  }, [programs, affiliates]);

  return <ResourceView config={config} />;
}
