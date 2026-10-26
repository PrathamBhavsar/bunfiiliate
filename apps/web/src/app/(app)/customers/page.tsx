"use client";

import { useMemo } from "react";
import { Facts, Section } from "@/components/admin-ui";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { type Customer, formatTime, ok, toPage, useLookups } from "@/lib/admin";
import { api } from "@/lib/api";
import type { ListFn } from "@/lib/use-resource";

const list: ListFn<Customer> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/customers", {
        signal,
        params: {
          query: {
            page,
            limit: pageSize,
            program_id: filters.program_id || undefined,
            affiliate_id: filters.affiliate_id || undefined,
          },
        },
      }),
    ),
  );

export default function Page() {
  const lookups = useLookups();

  const config = useMemo<ResourceConfig<Customer>>(() => {
    const { programOptions, affiliateOptions, programName, affiliateEmail } = lookups;
    const affiliate = (c: Customer) => (c.affiliate_id ? affiliateEmail(c.affiliate_id) : "—");
    return {
      title: "Customers",
      description: "People the tracker has seen buy, and the affiliate each one is attributed to.",
      list,
      get: (id, signal) => ok(api.GET("/api/v1/customers/{id}", { signal, params: { path: { id } } })),
      filters: [
        { id: "program_id", label: "Program", options: programOptions },
        { id: "affiliate_id", label: "Affiliate", options: affiliateOptions },
      ],
      columns: [
        { id: "external", header: "Customer ID", cell: (c) => <code className="font-medium">{c.external_id}</code> },
        { id: "program", header: "Program", cell: (c) => programName(c.program_id) },
        { id: "affiliate", header: "Affiliate", cell: affiliate },
        { id: "attributed", header: "Attributed", cell: (c) => (c.attributed_at ? formatTime(c.attributed_at) : "—") },
      ],
      detailTitle: (c) => c.external_id,
      detail: (c) => (
        <Section title="Customer">
          <Facts
            rows={[
              ["Customer ID", <code key="e">{c.external_id}</code>],
              ["Program", programName(c.program_id)],
              ["Affiliate", affiliate(c)],
              ["Attributed", c.attributed_at ? formatTime(c.attributed_at) : "—"],
              ["First click", c.first_click_id ? <code key="f">{c.first_click_id}</code> : "—"],
            ]}
          />
        </Section>
      ),
      empty: { title: "No customers", description: "Customers appear after the first tracked signup or sale." },
    };
  }, [lookups]);

  return <ResourceView config={config} />;
}
