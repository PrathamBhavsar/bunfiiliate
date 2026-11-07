"use client";

import { useQuery } from "@tanstack/react-query";
import { Eye, EyeOff, Pencil, Plus } from "lucide-react";
import { useState } from "react";
import { Facts, ListState, Section } from "@/components/admin-ui";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { FormDialog, type FormValues } from "@/components/form-dialog";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { formatTime, ok, refresh, required, toPage } from "@/lib/admin";
import { api } from "@/lib/api";
import { recall, remember } from "@/lib/revealed";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Endpoint = components["schemas"]["WebhookEndpoint"];
type Delivery = components["schemas"]["WebhookDelivery"];

const TOPICS = "conversion.created, conversion.approved, conversion.reversed, payout.paid";

const list: ListFn<Endpoint> = async ({ page, pageSize }, signal) =>
  toPage(await ok(api.GET("/api/v1/webhooks/endpoints", { signal, params: { query: { page, limit: pageSize } } })));

const get = (id: string, signal: AbortSignal) =>
  ok(api.GET("/api/v1/webhooks/endpoints/{id}", { signal, params: { path: { id } } }));

/** Comma-separated topics; empty = every topic. */
const parseEvents = (raw: string) =>
  raw
    .split(",")
    .map((e) => e.trim())
    .filter(Boolean);

const urlField = {
  name: "url",
  label: "Endpoint URL",
  type: "text" as const,
  required: true,
  placeholder: "https://example.com/hooks/bunfiiliate",
};
const eventsField = {
  name: "events",
  label: "Topics",
  hint: `Comma-separated. Leave empty for all. Known: ${TOPICS}.`,
};

function checkUrl(raw: string) {
  const url = required(raw, "URL");
  if (!/^https?:\/\/[^\s/]+/i.test(url)) throw new Error("URL must start with http:// or https://");
  return url;
}

// Delivery state lives in the existing columns: a scheduled retry is pending, otherwise 2xx means delivered.
const deliveryStatus = (d: Delivery) =>
  d.next_attempt_at ? "pending" : d.status_code !== null && d.status_code !== undefined && d.status_code < 300 && d.status_code >= 200 ? "delivered" : "dead";

function Deliveries({ endpointId }: { endpointId: string }) {
  const query = useQuery({
    queryKey: ["webhook-deliveries", endpointId],
    queryFn: async ({ signal }) =>
      (await ok(api.GET("/api/v1/webhooks/deliveries", { signal, params: { query: { endpoint_id: endpointId, page: 1, limit: 25 } } }))).items,
  });
  return (
    <Section title="Recent deliveries" action={<Button size="sm" onClick={() => query.refetch()}>Refresh</Button>}>
      <ListState query={query} empty="No deliveries yet. They appear when a matching event is dispatched.">
        {(items) =>
          items.map((d) => (
            <li key={d.id} className="grid gap-1 p-2 text-xs">
              <div className="flex items-center justify-between gap-2">
                <code>{d.topic}</code>
                <StatusBadge status={deliveryStatus(d)} />
              </div>
              <div className="flex items-center justify-between gap-2 text-muted">
                <span>
                  Attempt {d.attempt} · HTTP {d.status_code ?? "none"}
                  {d.next_attempt_at && ` · next ${formatTime(d.next_attempt_at)}`}
                </span>
                {deliveryStatus(d) !== "pending" && (
                  <ConfirmDialog
                    trigger={<Button size="sm">Redeliver</Button>}
                    title="Redeliver this event?"
                    description="Queues the same payload for another attempt, signed with the endpoint's current secret."
                    confirmLabel="Redeliver"
                    onConfirm={async () => {
                      await ok(api.POST("/api/v1/webhooks/deliveries/{id}/redeliver", { params: { path: { id: d.id } } }));
                      await refresh();
                    }}
                  />
                )}
              </div>
            </li>
          ))
        }
      </ListState>
    </Section>
  );
}

function Detail({ e }: { e: Endpoint }) {
  const [shown, setShown] = useState(false);
  const secret = recall("endpoint", e.id);
  return (
    <div className="grid gap-4">
      <Section
        title="Endpoint"
        action={
          <FormDialog
            trigger={
              <Button size="sm">
                <Pencil /> Edit
              </Button>
            }
            title="Edit endpoint"
            fields={[urlField, eventsField, { name: "active", label: "Status", type: "select", options: [{ value: "true", label: "Active" }, { value: "false", label: "Paused" }] }]}
            initial={{ url: e.url, events: e.events.join(", "), active: String(e.active) }}
            onSubmit={async (v: FormValues) => {
              await ok(
                api.PATCH("/api/v1/webhooks/endpoints/{id}", {
                  params: { path: { id: e.id } },
                  body: { url: checkUrl(v.url ?? ""), events: parseEvents(v.events ?? ""), active: v.active === "true" },
                }),
              );
              await refresh();
            }}
          />
        }
      >
        <Facts
          rows={[
            ["URL", <span key="u" className="break-all">{e.url}</span>],
            ["Topics", e.events.length ? e.events.join(", ") : "All"],
            ["Status", <StatusBadge key="s" status={e.active ? "active" : "paused"} tone={e.active ? undefined : "neutral"} />],
            [
              "Signing secret",
              <span key="k" className="inline-flex items-center gap-2">
                {secret ? (
                  <>
                    <code className="break-all">{shown ? secret : "••••••••••••"}</code>
                    <Button size="sm" aria-pressed={shown} onClick={() => setShown((s) => !s)}>
                      {shown ? <EyeOff /> : <Eye />}
                      {shown ? "Hide" : "Reveal"}
                    </Button>
                  </>
                ) : (
                  <span className="text-muted">Shown once, when the endpoint was created.</span>
                )}
              </span>,
            ],
          ]}
        />
        <p className="text-xs text-muted">
          Requests carry <code>X-Bf-Signature: t=&lt;unix&gt;,v1=&lt;HMAC-SHA256 of &quot;t.body&quot;&gt;</code>.
        </p>
      </Section>
      <Deliveries endpointId={e.id} />
      <Section title="Danger zone">
        <ConfirmDialog
          trigger={<Button variant="danger" size="sm" className="justify-self-start">Delete endpoint</Button>}
          title="Delete this endpoint?"
          description="Stops all deliveries to it and removes its delivery history."
          confirmLabel="Delete"
          destructive
          onConfirm={async () => {
            await ok(api.DELETE("/api/v1/webhooks/endpoints/{id}", { params: { path: { id: e.id } } }));
            await refresh();
          }}
        />
      </Section>
    </div>
  );
}

const config: ResourceConfig<Endpoint> = {
  title: "Webhooks",
  description: "Signed HTTP callbacks for conversion and payout events, retried with backoff.",
  list,
  get,
  columns: [
    { id: "url", header: "URL", className: "max-w-80 truncate", cell: (e) => e.url },
    { id: "events", header: "Topics", cell: (e) => (e.events.length ? e.events.join(", ") : "All") },
    { id: "status", header: "Status", cell: (e) => <StatusBadge status={e.active ? "active" : "paused"} tone={e.active ? undefined : "neutral"} /> },
  ],
  actions: (
    <FormDialog
      trigger={
        <Button variant="primary">
          <Plus /> New endpoint
        </Button>
      }
      title="New webhook endpoint"
      description="A signing secret is generated for you."
      fields={[urlField, eventsField]}
      submitLabel="Create"
      onSubmit={async (v) => {
        const created = await ok(api.POST("/api/v1/webhooks/endpoints", { body: { url: checkUrl(v.url ?? ""), events: parseEvents(v.events ?? "") } }));
        remember("endpoint", created.id, created.secret);
        await refresh();
      }}
    />
  ),
  detailTitle: (e) => e.url,
  detail: (e) => <Detail e={e} />,
  empty: { title: "No webhook endpoints", description: "Add an endpoint to receive signed events when conversions and payouts change." },
};

export default function Page() {
  return <ResourceView config={config} />;
}
