"use client";

import { Facts, Section } from "@/components/admin-ui";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { formatTime, ok, refresh, toPage } from "@/lib/admin";
import { api } from "@/lib/api";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Contact = components["schemas"]["ContactRequest"];

const STATUSES = ["new", "read", "closed"];
// The address is public input: only a plain single addr-spec becomes a mailto link (no `?`, `&`, `%`, spaces or commas).
const SAFE_EMAIL = /^[A-Za-z0-9._+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/;

const tone = (s: string) => (s === "new" ? ("accent" as const) : ("neutral" as const));

const list: ListFn<Contact> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/contact-requests", {
        signal,
        params: { query: { page, limit: pageSize, status: filters.status || undefined } },
      }),
    ),
  );

const config: ResourceConfig<Contact> = {
  title: "Contact requests",
  description: "Messages sent through the public contact form.",
  list,
  filters: [{ id: "status", label: "Status", options: STATUSES.map((s) => ({ value: s, label: s })) }],
  columns: [
    { id: "name", header: "Name", cell: (c) => c.name },
    { id: "email", header: "Email", cell: (c) => c.email },
    { id: "status", header: "Status", cell: (c) => <StatusBadge status={c.status} tone={tone(c.status)} /> },
    { id: "at", header: "Received", cell: (c) => formatTime(c.created_at) },
  ],
  detailTitle: (c) => c.name,
  detail: (c) => (
    <div className="grid gap-4">
      <Section title="Request">
        <Facts
          rows={[
            ["Name", c.name],
            ["Email", SAFE_EMAIL.test(c.email) ? <a key="e" className="underline" href={`mailto:${c.email}`}>{c.email}</a> : c.email],
            ["Received", formatTime(c.created_at)],
            ["Status", <StatusBadge key="s" status={c.status} tone={tone(c.status)} />],
          ]}
        />
        <p className="rounded-md border border-border bg-subtle p-2 text-xs whitespace-pre-wrap">{c.message}</p>
      </Section>
      <Section title="Mark as">
        <div className="flex gap-1">
          {STATUSES.map((s) => (
            <Button
              key={s}
              size="sm"
              disabled={s === c.status}
              onClick={async () => {
                await ok(api.PATCH("/api/v1/contact-requests/{id}", { params: { path: { id: c.id } }, body: { status: s } }));
                await refresh();
              }}
            >
              {s}
            </Button>
          ))}
        </div>
      </Section>
    </div>
  ),
  empty: { title: "No contact requests", description: "Messages from the public contact form show up here." },
};

export default function Page() {
  return <ResourceView config={config} />;
}
