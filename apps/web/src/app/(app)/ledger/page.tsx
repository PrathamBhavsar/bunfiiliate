"use client";

import { useMemo, useState } from "react";
import { Facts, Section } from "@/components/admin-ui";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { Button } from "@/components/ui/button";
import { type Account, type Balance, type Entry, formatCents, formatTime, ok, toPage, useLookups } from "@/lib/admin";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import type { ListFn } from "@/lib/use-resource";

type View = "entries" | "balances";
type BalanceRow = Balance & { id: string };

const ACCOUNTS: Account[] = ["expense", "aff_pending", "aff_payable", "cash"];

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const listEntries: ListFn<Entry> = async ({ page, pageSize, search, filters }, signal) => {
  if (search && !UUID.test(search)) throw new Error("Transaction ID must be a UUID");
  return toPage(
    await ok(
      api.GET("/api/v1/ledger/entries", {
        signal,
        params: {
          query: {
            page,
            limit: pageSize,
            program_id: filters.program_id || undefined,
            affiliate_id: filters.affiliate_id || undefined,
            account: (filters.account || undefined) as Account | undefined,
            txn_id: search || undefined,
          },
        },
      }),
    ),
  );
};

const listBalances: ListFn<BalanceRow> = async ({ page, pageSize, filters }, signal) => {
  const { items, total } = await ok(
    api.GET("/api/v1/ledger/balances", {
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
  );
  return toPage({ items: items.map((b) => ({ ...b, id: `${b.program_id}:${b.affiliate_id}` })), total });
};

function ViewSwitch({ view, onChange }: { view: View; onChange: (v: View) => void }) {
  return (
    <div role="group" aria-label="Ledger view" className="flex gap-1">
      {(["entries", "balances"] as const).map((v) => (
        <Button
          key={v}
          size="sm"
          aria-pressed={v === view}
          className={cn("capitalize", v === view && "bg-subtle")}
          onClick={() => onChange(v)}
        >
          {v}
        </Button>
      ))}
    </div>
  );
}

export default function Page() {
  const [view, setView] = useState<View>("entries");
  const lookups = useLookups();

  const entries = useMemo<ResourceConfig<Entry>>(() => {
    const { programOptions, affiliateOptions, programName, affiliateEmail } = lookups;
    const affiliate = (e: Entry) => (e.affiliate_id ? affiliateEmail(e.affiliate_id) : "—");
    return {
      title: "Ledger",
      description: "Double-entry legs, newest first. Debits are positive, credits negative; each transaction sums to zero.",
      actions: <ViewSwitch view="entries" onChange={setView} />,
      list: listEntries,
      searchPlaceholder: "Transaction ID",
      filters: [
        { id: "program_id", label: "Program", options: programOptions },
        { id: "affiliate_id", label: "Affiliate", options: affiliateOptions },
        { id: "account", label: "Account", options: ACCOUNTS.map((a) => ({ value: a, label: a.replace("_", " ") })) },
      ],
      columns: [
        { id: "created", header: "Posted", cell: (e) => formatTime(e.created_at) },
        { id: "account", header: "Account", cell: (e) => e.account.replace("_", " ") },
        { id: "amount", header: "Amount", align: "right", cell: (e) => formatCents(e.amount_cents, e.currency) },
        { id: "program", header: "Program", cell: (e) => programName(e.program_id) },
        { id: "affiliate", header: "Affiliate", cell: affiliate },
        { id: "txn", header: "Txn", cell: (e) => <code title={e.txn_id}>{e.txn_id.slice(0, 8)}</code> },
      ],
      detailTitle: (e) => `Entry in txn ${e.txn_id.slice(0, 8)}`,
      detail: (e) => (
        <Section title="Entry">
          <Facts
            rows={[
              ["Account", e.account.replace("_", " ")],
              ["Amount", formatCents(e.amount_cents, e.currency)],
              ["Posted", formatTime(e.created_at)],
              ["Program", programName(e.program_id)],
              ["Affiliate", affiliate(e)],
              ["Transaction", <code key="t">{e.txn_id}</code>],
              ["Conversion", e.conversion_id ? <code key="c">{e.conversion_id}</code> : "—"],
              ["Payout", e.payout_id ? <code key="p">{e.payout_id}</code> : "—"],
            ]}
          />
        </Section>
      ),
      empty: { title: "No ledger entries", description: "Entries are posted when conversions are tracked, approved, reversed or paid." },
    };
  }, [lookups]);

  const balances = useMemo<ResourceConfig<BalanceRow>>(() => {
    const { programOptions, affiliateOptions, programName, affiliateEmail } = lookups;
    return {
      title: "Ledger",
      description: "What each affiliate is owed per program, derived from the ledger. Payable is negative after a clawback.",
      actions: <ViewSwitch view="balances" onChange={setView} />,
      list: listBalances,
      filters: [
        { id: "program_id", label: "Program", options: programOptions },
        { id: "affiliate_id", label: "Affiliate", options: affiliateOptions },
      ],
      columns: [
        { id: "affiliate", header: "Affiliate", cell: (b) => affiliateEmail(b.affiliate_id) },
        { id: "program", header: "Program", cell: (b) => programName(b.program_id) },
        { id: "pending", header: "Pending", align: "right", cell: (b) => formatCents(b.pending_cents, b.currency) },
        { id: "payable", header: "Payable", align: "right", cell: (b) => formatCents(b.payable_cents, b.currency) },
      ],
      empty: { title: "No balances", description: "Balances appear once an affiliate earns commission." },
    };
  }, [lookups]);

  return view === "entries" ? <ResourceView key="entries" config={entries} /> : <ResourceView key="balances" config={balances} />;
}
