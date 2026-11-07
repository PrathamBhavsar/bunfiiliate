"use client";

import { useMutation } from "@tanstack/react-query";
import Link from "next/link";
import { useMemo, useState, type FormEvent } from "react";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Label, Select, Textarea } from "@/components/ui/input";
import { formatTime, ok, refresh, toPage, useLookups } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Inbound = components["schemas"]["InboundEventRow"];
type Source = "generic" | "stripe" | "paddle";

const SOURCES: Source[] = ["generic", "stripe", "paddle"];

/** Prefilled bodies, one per source. Edit freely: the server signs whatever is sent. */
const FIXTURES: Record<Source, object> = {
  generic: { type: "sale", customer: "cus_test_1", external_id: "test-order-1", amount_cents: 4900, currency: "USD" },
  stripe: { id: "evt_test_1", type: "charge.refunded", data: { object: { id: "ch_test_1", amount_refunded: 4900 } } },
  paddle: { event_id: "evt_test_1", event_type: "adjustment.created", data: { id: "adj_test_1", action: "refund" } },
};

const pretty = (s: Source) => JSON.stringify(FIXTURES[s], null, 2);

const list: ListFn<Inbound> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/webhooks/inbound", {
        signal,
        params: { query: { page, limit: pageSize, program_id: filters.program_id || undefined, source: filters.source || undefined } },
      }),
    ),
  );

const state = (e: Inbound) => (e.error ? "failed" : e.processed_at ? "processed" : "queued");

function Tester() {
  const { programOptions } = useLookups();
  const [source, setSource] = useState<Source>("generic");
  const [payload, setPayload] = useState(pretty("generic"));
  const [programId, setProgramId] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const send = useMutation({
    mutationFn: async () => {
      let body: unknown;
      try {
        body = JSON.parse(payload);
      } catch {
        throw new Error("Payload is not valid JSON.");
      }
      if (!programId) throw new Error("Choose a program.");
      return ok(api.POST("/api/v1/webhooks/test", { body: { program_id: programId, source, payload: body as never } }));
    },
    onSuccess: () => refresh(),
  });

  function submit(e: FormEvent) {
    e.preventDefault();
    setFormError(null);
    send.mutate(undefined, { onError: (err) => setFormError(errorMessage(err)) });
  }

  return (
    <form onSubmit={submit} className="grid max-w-2xl gap-3 rounded-lg border border-border bg-surface p-3">
      <p className="text-xs text-muted">
        Signs the payload with the program&apos;s own secret and delivers it to the real inbound endpoint, so you can check a
        Stripe, Paddle or generic secret without a provider account.
      </p>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="grid gap-1">
          <Label htmlFor="wt-program">Test program</Label>
          <Select id="wt-program" value={programId} onChange={(e) => setProgramId(e.target.value)}>
            <option value="">Choose…</option>
            {programOptions.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </Select>
        </div>
        <div className="grid gap-1">
          <Label htmlFor="wt-source">Event source</Label>
          <Select
            id="wt-source"
            value={source}
            onChange={(e) => {
              const next = e.target.value as Source;
              setSource(next);
              setPayload(pretty(next));
            }}
          >
            {SOURCES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
        </div>
      </div>
      <div className="grid gap-1">
        <Label htmlFor="wt-payload">JSON payload</Label>
        <Textarea
          id="wt-payload"
          rows={9}
          spellCheck={false}
          className="font-mono text-xs"
          value={payload}
          onChange={(e) => setPayload(e.target.value)}
        />
      </div>
      <div className="flex items-center gap-2">
        <Button type="submit" variant="primary" disabled={send.isPending}>
          {send.isPending ? "Sending…" : "Send test"}
        </Button>
      </div>
      {formError && (
        <p role="alert" className="text-xs text-danger">
          {formError}
        </p>
      )}
      {send.data && (
        <div role="status" data-testid="test-result" className="grid gap-1 text-xs">
          <p>
            Tracker answered <strong data-testid="test-status">HTTP {send.data.status}</strong>
          </p>
          <pre className="max-h-48 overflow-auto rounded border border-border bg-subtle p-2 whitespace-pre-wrap">
            {send.data.body || "(empty body)"}
          </pre>
        </div>
      )}
    </form>
  );
}

export default function Page() {
  const { programOptions } = useLookups();
  const config = useMemo<ResourceConfig<Inbound>>(
    () => ({
      title: "Received webhooks",
      description: "Inbound deliveries and how they were processed. Replay one to run it through the worker again.",
      list,
      filters: [
        { id: "program_id", label: "Program", options: programOptions },
        { id: "source", label: "Source", options: SOURCES.map((s) => ({ value: s, label: s })) },
      ],
      columns: [
        { id: "received", header: "Received", cell: (e) => formatTime(e.received_at) },
        { id: "source", header: "Source", cell: (e) => e.source },
        { id: "external", header: "Event id", cell: (e) => <code>{e.external_id}</code> },
        { id: "state", header: "State", cell: (e) => <StatusBadge status={state(e)} /> },
        { id: "error", header: "Error", className: "max-w-64 truncate", cell: (e) => e.error ?? "—" },
        {
          id: "replay",
          header: "Replay",
          cell: (e) =>
            e.processed_at ? (
              <ReplayButton id={e.id} />
            ) : (
              <span className="text-muted">queued</span>
            ),
        },
      ],
      empty: { title: "Nothing received yet", description: "Send a test above, or point a provider at your inbound URL." },
    }),
    [programOptions],
  );

  return (
    <div className="grid gap-4">
      <section aria-labelledby="page-title" className="grid gap-3">
        <header className="flex flex-wrap items-start justify-between gap-2">
          <div>
            <h1 id="page-title" className="text-base font-semibold">
              Webhook tester
            </h1>
            <p className="text-xs text-muted">Send a signed test event and watch it arrive.</p>
          </div>
          <Link href="/webhooks" className="text-xs underline underline-offset-2">
            Back to endpoints
          </Link>
        </header>
        <Tester />
      </section>
      <ResourceView config={config} headingLevel={2} />
    </div>
  );
}

function ReplayButton({ id }: { id: string }) {
  const replay = useMutation({
    mutationFn: () => ok(api.POST("/api/v1/webhooks/inbound/{id}/replay", { params: { path: { id } } })),
    onSuccess: () => refresh(),
  });
  return (
    <span className="inline-flex items-center gap-2">
      <Button size="sm" disabled={replay.isPending} onClick={() => replay.mutate()}>
        {replay.isPending ? "Queuing…" : "Replay"}
      </Button>
      {replay.isError && (
        <span role="alert" className="text-danger">
          {errorMessage(replay.error)}
        </span>
      )}
    </span>
  );
}
