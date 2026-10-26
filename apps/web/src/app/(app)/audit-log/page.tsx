"use client";

import { Facts, Section } from "@/components/admin-ui";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { formatTime, listParams, ok, toPage } from "@/lib/admin";
import { api } from "@/lib/api";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Record = components["schemas"]["AuditRecord"];

// Entity types the API writes audit entries for.
const ENTITIES = [
  "affiliate",
  "contact_request",
  "conversion_type",
  "email_template",
  "enrollment",
  "member",
  "plan",
  "program",
  "referral_code",
  "settings",
  "site",
  "user",
  "webhook_delivery",
  "webhook_endpoint",
  "workspace",
];

const list: ListFn<Record> = async ({ page, pageSize, search, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/audit-log", {
        signal,
        params: listParams({}, { page, limit: pageSize, entity: filters.entity || null, action: search || null }),
      }),
    ),
  );

const config: ResourceConfig<Record> = {
  title: "Audit log",
  description: "Who changed what, newest first. Secrets are never recorded.",
  list,
  searchPlaceholder: "Exact action, e.g. program.create",
  filters: [{ id: "entity", label: "Entity", options: ENTITIES.map((e) => ({ value: e, label: e })) }],
  columns: [
    { id: "at", header: "When", cell: (r) => formatTime(r.at) },
    { id: "action", header: "Action", cell: (r) => <code>{r.action}</code> },
    { id: "entity", header: "Entity", cell: (r) => r.entity },
    { id: "entity_id", header: "Entity ID", className: "max-w-48 truncate font-mono text-xs", cell: (r) => r.entity_id },
    { id: "actor", header: "Actor", className: "max-w-32 truncate font-mono text-xs", cell: (r) => r.actor_id ?? "system / public" },
  ],
  detailTitle: (r) => r.action,
  detail: (r) => (
    <div className="grid gap-4">
      <Section title="Entry">
        <Facts
          rows={[
            ["When", formatTime(r.at)],
            ["Action", <code key="a">{r.action}</code>],
            ["Entity", `${r.entity} ${r.entity_id}`],
            ["Actor", r.actor_id ?? "system / public"],
          ]}
        />
      </Section>
      <Section title="Change">
        {r.diff ? (
          <pre className="max-h-96 overflow-auto rounded-md border border-border bg-subtle p-2 text-xs">{JSON.stringify(r.diff, null, 2)}</pre>
        ) : (
          <p className="text-xs text-muted">No details recorded.</p>
        )}
      </Section>
    </div>
  ),
  empty: { title: "No audit entries", description: "Changes to programs, affiliates, webhooks and settings are recorded here." },
};

export default function Page() {
  return <ResourceView config={config} />;
}
