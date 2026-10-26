"use client";

import { Check, Copy, Plus } from "lucide-react";
import { useMemo, useState } from "react";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { FormDialog } from "@/components/form-dialog";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { Button } from "@/components/ui/button";
import { ok, refresh, required, toPage, usePrograms, type ReferralCode } from "@/lib/admin";
import { api } from "@/lib/api";
import type { ListFn } from "@/lib/use-resource";

type Row = ReferralCode & { id: string };

// The tracker serves /r/:code on the same origin as the app.
const referralUrl = (code: string) => `${window.location.origin}/r/${code}`;

const list: ListFn<Row> = async ({ page, pageSize }, signal) => {
  const { items, total } = await ok(
    api.GET("/api/v1/me/referral-codes", { signal, params: { query: { page, limit: pageSize } } }),
  );
  return toPage({ items: items.map((c) => ({ ...c, id: c.code })), total });
};

function CopyLink({ code }: { code: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  async function copy() {
    try {
      await navigator.clipboard.writeText(referralUrl(code));
      setState("copied");
    } catch {
      setState("failed");
    }
    setTimeout(() => setState("idle"), 2000);
  }
  return (
    <Button size="sm" aria-label={`Copy link for ${code}`} onClick={copy}>
      {state === "copied" ? <Check /> : <Copy />}
      {state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : "Copy link"}
    </Button>
  );
}

export default function Page() {
  const programs = usePrograms().data;

  const config = useMemo<ResourceConfig<Row>>(() => {
    const programName = (id: string) => programs?.find((p) => p.id === id)?.name ?? id;
    return {
      title: "Links",
      description: "Your referral codes. Share a link to attribute clicks and sales to you.",
      list,
      columns: [
        { id: "code", header: "Code", cell: (c) => <code className="font-medium">{c.code}</code> },
        { id: "program", header: "Program", cell: (c) => programName(c.program_id) },
        { id: "landing", header: "Landing URL", className: "break-all", cell: (c) => c.landing_url },
        {
          id: "actions",
          header: "Actions",
          cell: (c) => (
            <div className="flex gap-1.5">
              <CopyLink code={c.code} />
              <ConfirmDialog
                trigger={
                  <Button size="sm" aria-label={`Deactivate ${c.code}`}>
                    Deactivate
                  </Button>
                }
                title={`Deactivate ${c.code}?`}
                description="The link stops tracking right away. Clicks already recorded keep their attribution."
                confirmLabel="Deactivate"
                destructive
                onConfirm={async () => {
                  await ok(api.DELETE("/api/v1/me/referral-codes/{code}", { params: { path: { code: c.code } } }));
                  await refresh();
                }}
              />
            </div>
          ),
        },
      ],
      actions: (
        <FormDialog
          trigger={
            <Button variant="primary">
              <Plus /> New link
            </Button>
          }
          title="New link"
          description="You must be active and enrolled in the program."
          fields={[
            {
              name: "program_id",
              label: "Program",
              type: "select",
              required: true,
              options: [{ value: "", label: "Choose…" }, ...(programs ?? []).map((p) => ({ value: p.id, label: p.name }))],
            },
            { name: "code", label: "Code", hint: "Optional. 3-32 characters of A-Z a-z 0-9 - _. Generated when empty." },
            { name: "landing_url", label: "Landing URL", hint: "Optional. Must be on one of the program's sites. Defaults to its first site." },
          ]}
          submitLabel="Create"
          onSubmit={async (v) => {
            const code = (v.code ?? "").trim();
            if (code && !/^[A-Za-z0-9_-]{3,32}$/.test(code)) throw new Error("Code must be 3-32 characters of A-Z a-z 0-9 - _");
            const landing = (v.landing_url ?? "").trim();
            if (landing && !/^https?:\/\/[^\s/]+/i.test(landing)) throw new Error("Landing URL must start with http:// or https://");
            await ok(
              api.POST("/api/v1/me/referral-codes", {
                body: { program_id: required(v.program_id ?? "", "Program"), code: code || undefined, landing_url: landing || undefined },
              }),
            );
            await refresh();
          }}
        />
      ),
      empty: { title: "No links yet", description: "Create a link to get a shareable URL that tracks your referrals." },
    };
  }, [programs]);

  return <ResourceView config={config} />;
}
