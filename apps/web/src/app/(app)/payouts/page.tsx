"use client";

import { useQuery } from "@tanstack/react-query";
import { Download, Plus } from "lucide-react";
import { usePathname, useRouter } from "next/navigation";
import { useMemo, useState } from "react";
import { Facts, ListState, Section } from "@/components/admin-ui";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { FormDialog } from "@/components/form-dialog";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { type Batch, formatCents, formatTime, ok, refresh, required, toPage, useLookups } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";
import type { ListFn } from "@/lib/use-resource";
import type { components } from "@bunfiiliate/sdk";

const REQUEST_STATUSES = ["pending", "approved", "rejected", "paid"];
const STATUSES = ["draft", "approved", "paid", "cancelled"];

const list: ListFn<Batch> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/payouts/batches", {
        signal,
        params: {
          query: { page, limit: pageSize, program_id: filters.program_id || undefined, status: filters.status || undefined },
        },
      }),
    ),
  );

type Action = "approve" | "mark-paid" | "cancel";

const ACTIONS: Record<Action, { label: string; destructive?: boolean; describe: (b: Batch) => string }> = {
  approve: {
    label: "Approve",
    describe: (b) =>
      `Approves ${b.payout_count} payouts totalling ${formatCents(b.total_cents, b.currency)}. The batch can then be exported and paid, but no longer cancelled.`,
  },
  "mark-paid": {
    label: "Mark paid",
    describe: (b) =>
      `Confirm you have sent ${formatCents(b.total_cents, b.currency)} to ${b.payout_count} affiliates. This moves their payable balance to cash in the ledger and cannot be undone.`,
  },
  cancel: {
    label: "Cancel batch",
    destructive: true,
    describe: () => `Cancels this draft and releases its conversions so the next batch can pick them up.`,
  },
};

const ALLOWED: Record<string, Action[] | undefined> = {
  draft: ["approve", "cancel"],
  approved: ["mark-paid"],
};

const allowed = (status: string) => ALLOWED[status] ?? [];

/** Fetch the CSV with the session cookie and hand it to the browser as a file; errors show inline. */
function CsvButton({ id }: { id: string }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function download() {
    setPending(true);
    setError(null);
    try {
      const blob = await ok(
        api.GET("/api/v1/payouts/batches/{id}/export.csv", { params: { path: { id } }, parseAs: "blob" }),
      );
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `payouts-${id}.csv`;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPending(false);
    }
  }

  return (
    <>
      <Button size="sm" disabled={pending} onClick={download}>
        <Download /> {pending ? "Preparing…" : "Download CSV"}
      </Button>
      {error && (
        <p role="alert" className="basis-full text-xs text-danger">
          {error}
        </p>
      )}
    </>
  );
}

const RAIL_LABELS: Record<string, string> = {
  manual_csv: "Manual (CSV, you send the money)",
  paypal_payouts: "PayPal Payouts",
  wise: "Wise",
  mock: "Mock (testing)",
};

/** Rails the workspace can pick; `manual_csv` is always there. Credentials never reach the browser. */
function useRails() {
  return useQuery({
    queryKey: ["payout-rails"],
    queryFn: ({ signal }) => ok(api.GET("/api/v1/payouts/rails", { signal })),
  });
}

type Outcome = { paid: number; submitted: number; failed: number; batch_paid: boolean };

function outcomeText(o: Outcome) {
  return `${o.paid} paid, ${o.submitted} submitted, ${o.failed} failed${o.batch_paid ? ". Batch is now paid." : "."}`;
}

/** Mark paid with a rail: manual keeps the old confirm-and-flip; a provider rail sends each line. */
function MarkPaidDialog({ batch: b, onDone }: { batch: Batch; onDone: (o: Outcome) => void }) {
  const rails = useRails();
  const options = (rails.data ?? [{ rail: "manual_csv", available: true }])
    .filter((r) => r.available)
    .map((r) => ({ value: r.rail, label: RAIL_LABELS[r.rail] ?? r.rail }));
  return (
    <FormDialog
      trigger={
        <Button size="sm" variant="primary">
          {ACTIONS["mark-paid"].label}
        </Button>
      }
      title="Mark paid?"
      description={`${ACTIONS["mark-paid"].describe(b)} Choosing a provider rail sends each payout through it first; the ledger posts only when the provider confirms.`}
      fields={[{ name: "rail", label: "Payout rail", type: "select", options }]}
      initial={{ rail: "manual_csv" }}
      submitLabel="Mark paid"
      onSubmit={async (v) => {
        const rail = v.rail || "manual_csv";
        const res = await ok(
          api.POST("/api/v1/payouts/batches/{id}/mark-paid", { params: { path: { id: b.id } }, body: { rail } as never }),
        );
        onDone((res as { outcome: Outcome }).outcome);
        await refresh();
      }}
    />
  );
}

function ReconcileButton({ id, onDone }: { id: string; onDone: (o: Outcome) => void }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <>
      <Button
        size="sm"
        disabled={pending}
        onClick={async () => {
          setPending(true);
          setError(null);
          try {
            const res = await ok(api.POST("/api/v1/payouts/batches/{id}/reconcile", { params: { path: { id } } }));
            onDone((res as { outcome: Outcome }).outcome);
            await refresh();
          } catch (e) {
            setError(errorMessage(e));
          } finally {
            setPending(false);
          }
        }}
      >
        {pending ? "Checking…" : "Reconcile"}
      </Button>
      {error && (
        <p role="alert" className="basis-full text-xs text-danger">
          {error}
        </p>
      )}
    </>
  );
}

function BatchPanel({ batch: b }: { batch: Batch }) {
  const { programName } = useLookups();
  const [outcome, setOutcome] = useState<string | null>(null);
  const payouts = useQuery({
    queryKey: ["payout-batch", b.id],
    queryFn: ({ signal }) => ok(api.GET("/api/v1/payouts/batches/{id}", { signal, params: { path: { id: b.id } } })),
    select: (d) => d.payouts,
  });

  return (
    <div className="grid gap-4">
      <Section title="Batch">
        <Facts
          rows={[
            ["Status", <StatusBadge key="s" status={b.status} />],
            ["Program", programName(b.program_id)],
            ["Total", formatCents(b.total_cents, b.currency)],
            ["Payouts", String(b.payout_count)],
            ["Created", formatTime(b.created_at)],
            ["Created by", <code key="u">{b.created_by}</code>],
            ["Approved by", b.approved_by ? <code key="a">{b.approved_by}</code> : "—"],
            ["Paid", b.paid_at ? formatTime(b.paid_at) : "—"],
          ]}
        />
      </Section>
      <Section title="Actions">
        <div className="flex flex-wrap gap-2">
          {allowed(b.status).map((action) =>
            action === "mark-paid" ? (
              <MarkPaidDialog key={action} batch={b} onDone={(o) => setOutcome(outcomeText(o))} />
            ) : (
            <ConfirmDialog
              key={action}
              trigger={
                <Button size="sm" variant={action === "cancel" ? "secondary" : "primary"}>
                  {ACTIONS[action].label}
                </Button>
              }
              title={`${ACTIONS[action].label}?`}
              description={ACTIONS[action].describe(b)}
              confirmLabel={ACTIONS[action].label}
              destructive={ACTIONS[action].destructive}
              onConfirm={async () => {
                await ok(api.POST(`/api/v1/payouts/batches/{id}/${action}`, { params: { path: { id: b.id } } }));
                await refresh();
              }}
            />
            ),
          )}
          {b.status === "approved" && <ReconcileButton id={b.id} onDone={(o) => setOutcome(outcomeText(o))} />}
          <CsvButton id={b.id} />
        </div>
        {outcome && (
          <p role="status" data-testid="rail-outcome" className="text-xs">
            {outcome}
          </p>
        )}
      </Section>
      <Section title="Payouts">
        <ListState query={payouts} empty="This batch has no payout lines.">
          {(lines) =>
            lines.map((p) => (
              <li key={p.id} className="flex items-center justify-between gap-2 px-2.5 py-2 text-xs">
                <span className="min-w-0 break-words">{p.email}</span>
                <span className="flex shrink-0 items-center gap-2">
                  <StatusBadge status={p.status} />
                  {(b.status === "approved" || b.status === "paid") && (
                    <a
                      href={`/api/v1/payouts/${p.id}/invoice.html`}
                      target="_blank"
                      rel="noopener"
                      className="underline underline-offset-2"
                    >
                      Invoice
                    </a>
                  )}
                  <span className="tabular-nums">{formatCents(p.amount_cents, b.currency)}</span>
                </span>
              </li>
            ))
          }
        </ListState>
      </Section>
    </div>
  );
}

type PayoutRequest = components["schemas"]["RequestView"];

const requestList: ListFn<PayoutRequest> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/payout-requests", {
        signal,
        params: { query: { page, limit: pageSize, status: filters.status || undefined } },
      }),
    ),
  );

/** Approve or reject one pending request. Approve drafts a batch sized from the current balance. */
function RequestActions({ request: r, label }: { request: PayoutRequest; label: string }) {
  const [note, setNote] = useState<string | null>(null);
  if (r.status !== "pending") return <span className="text-xs text-muted">—</span>;
  return (
    <span className="flex flex-wrap items-center gap-2">
      <ConfirmDialog
        trigger={
          <Button size="sm" variant="primary" aria-label={`Approve request from ${label}`}>
            Approve
          </Button>
        }
        title="Approve payout request?"
        description={`Drafts a payout batch for ${label}, sized from their current payable balance (they asked for ${formatCents(r.requested_cents, r.currency)}). Mark it paid from the batch list.`}
        confirmLabel="Approve"
        onConfirm={async () => {
          const res = await ok(api.POST("/api/v1/payout-requests/{id}/approve", { params: { path: { id: r.id } } }));
          setNote(`Drafted ${formatCents(res.amount_cents, r.currency)}`);
          await refresh();
        }}
      />
      <FormDialog
        trigger={
          <Button size="sm" aria-label={`Reject request from ${label}`}>
            Reject
          </Button>
        }
        title="Reject payout request"
        description="The affiliate sees this reason."
        fields={[{ name: "reason", label: "Reason", type: "textarea", required: true, hint: "Up to 500 characters." }]}
        submitLabel="Reject"
        onSubmit={async (v) => {
          await ok(
            api.POST("/api/v1/payout-requests/{id}/reject", {
              params: { path: { id: r.id } },
              body: { reason: required(v.reason ?? "", "Reason") },
            }),
          );
          await refresh();
        }}
      />
      {note && <span role="status" className="text-xs">{note}</span>}
    </span>
  );
}

function PayoutRequests() {
  const lookups = useLookups();
  const config = useMemo<ResourceConfig<PayoutRequest>>(() => {
    const { programName, affiliateEmail } = lookups;
    return {
      title: "Payout requests",
      description: "Affiliates ask to be paid. Approve drafts a payout from their balance; reject sends them a reason.",
      list: requestList,
      filters: [{ id: "status", label: "Status", options: REQUEST_STATUSES.map((s) => ({ value: s, label: s })) }],
      columns: [
        { id: "created", header: "Requested", cell: (r) => formatTime(r.created_at) },
        { id: "affiliate", header: "Affiliate", cell: (r) => affiliateEmail(r.affiliate_id) },
        { id: "program", header: "Program", cell: (r) => programName(r.program_id) },
        { id: "amount", header: "Amount", align: "right", className: "tabular-nums", cell: (r) => formatCents(r.requested_cents, r.currency) },
        { id: "status", header: "Status", cell: (r) => <StatusBadge status={r.status} /> },
        { id: "reason", header: "Reason", className: "max-w-48 truncate", cell: (r) => r.rejection_reason ?? "—" },
        { id: "actions", header: "Actions", cell: (r) => <RequestActions request={r} label={affiliateEmail(r.affiliate_id)} /> },
      ],
      empty: { title: "No payout requests", description: "Requests from affiliates appear here." },
    };
  }, [lookups]);
  return <ResourceView config={config} headingLevel={2} />;
}

export default function Page() {
  const router = useRouter();
  const pathname = usePathname();
  const lookups = useLookups();

  const config = useMemo<ResourceConfig<Batch>>(() => {
    const { programOptions, programName } = lookups;
    return {
      title: "Payouts",
      description: "Pay affiliates in batches: draft, approve, send the money, mark paid.",
      list,
      get: (id, signal) =>
        ok(api.GET("/api/v1/payouts/batches/{id}", { signal, params: { path: { id } } })),
      filters: [
        { id: "program_id", label: "Program", options: programOptions },
        { id: "status", label: "Status", options: STATUSES.map((s) => ({ value: s, label: s })) },
      ],
      columns: [
        { id: "created", header: "Created", cell: (b) => formatTime(b.created_at) },
        { id: "program", header: "Program", cell: (b) => programName(b.program_id) },
        { id: "status", header: "Status", cell: (b) => <StatusBadge status={b.status} /> },
        { id: "count", header: "Payouts", align: "right", cell: (b) => b.payout_count },
        { id: "total", header: "Total", align: "right", cell: (b) => formatCents(b.total_cents, b.currency) },
        { id: "id", header: "Batch", cell: (b) => <code title={b.id}>{b.id.slice(0, 8)}</code> },
      ],
      actions: (
        <FormDialog
          trigger={
            <Button variant="primary">
              <Plus /> New batch
            </Button>
          }
          title="New payout batch"
          description="Drafts a payout for every active affiliate in the program who is owed at least their minimum."
          fields={[
            {
              name: "program_id",
              label: "Program",
              type: "select",
              required: true,
              options: [{ value: "", label: "Choose…" }, ...programOptions],
            },
          ]}
          submitLabel="Create draft"
          onSubmit={async (v) => {
            const batch = await ok(
              api.POST("/api/v1/payouts/batches", { body: { program_id: required(v.program_id ?? "", "Program") } }),
            );
            await refresh();
            router.replace(`${pathname}?id=${encodeURIComponent(batch.id)}`, { scroll: false });
          }}
        />
      ),
      detailTitle: (b) => `Batch ${b.id.slice(0, 8)}`,
      detail: (b) => <BatchPanel batch={b} />,
      empty: { title: "No payout batches", description: "Create a batch once affiliates have approved commission to pay out." },
    };
  }, [lookups, router, pathname]);

  return (
    <div className="grid gap-4">
      <ResourceView config={config} />
      <PayoutRequests />
    </div>
  );
}
