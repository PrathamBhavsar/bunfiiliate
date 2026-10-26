"use client";

import { Pencil, Plus } from "lucide-react";
import { Facts, Section } from "@/components/admin-ui";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { FormDialog } from "@/components/form-dialog";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { Button } from "@/components/ui/button";
import { listParams, ok, refresh, required, toPage } from "@/lib/admin";
import { api } from "@/lib/api";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Template = components["schemas"]["EmailTemplate"];

// The keys the dispatcher sends mail for; the API rejects any other.
const KEYS = [
  { value: "conversion_approved", label: "conversion_approved (commission approved)" },
  { value: "payout_paid", label: "payout_paid (payout sent)" },
];
const VARS_HINT = "Variables like {{commission}}, {{amount}}, {{currency}}. Unknown names render empty.";

const list: ListFn<Template> = async ({ page, pageSize }, signal) =>
  toPage(await ok(api.GET("/api/v1/email-templates", { signal, params: listParams({}, { page, limit: pageSize }) })));

const get = (id: string, signal: AbortSignal) =>
  ok(api.GET("/api/v1/email-templates/{id}", { signal, params: { path: { id } } }));

const subjectField = { name: "subject", label: "Subject", required: true, hint: VARS_HINT };
const bodyField = { name: "body", label: "Body", type: "textarea" as const, required: true };

const config: ResourceConfig<Template> = {
  title: "Email templates",
  description: "Override the built-in text of affiliate emails. A key without a template uses the default.",
  list,
  get,
  columns: [
    { id: "key", header: "Key", cell: (t) => <code className="font-medium">{t.key}</code> },
    { id: "subject", header: "Subject", className: "max-w-96 truncate", cell: (t) => t.subject },
  ],
  actions: (
    <FormDialog
      trigger={
        <Button variant="primary">
          <Plus /> New template
        </Button>
      }
      title="New email template"
      description="One template per key."
      fields={[{ name: "key", label: "Key", type: "select", required: true, options: KEYS }, subjectField, bodyField]}
      submitLabel="Create"
      onSubmit={async (v) => {
        await ok(
          api.POST("/api/v1/email-templates", {
            body: { key: required(v.key ?? "", "Key"), subject: required(v.subject ?? "", "Subject"), body: required(v.body ?? "", "Body") },
          }),
        );
        await refresh();
      }}
    />
  ),
  detailTitle: (t) => t.key,
  detail: (t) => (
    <div className="grid gap-4">
      <Section
        title="Template"
        action={
          <FormDialog
            trigger={
              <Button size="sm">
                <Pencil /> Edit
              </Button>
            }
            title={`Edit ${t.key}`}
            fields={[subjectField, bodyField]}
            initial={{ subject: t.subject, body: t.body }}
            onSubmit={async (v) => {
              await ok(
                api.PATCH("/api/v1/email-templates/{id}", {
                  params: { path: { id: t.id } },
                  body: { subject: required(v.subject ?? "", "Subject"), body: required(v.body ?? "", "Body") },
                }),
              );
              await refresh();
            }}
          />
        }
      >
        <Facts rows={[["Key", <code key="k">{t.key}</code>], ["Subject", t.subject]]} />
        <pre className="max-h-72 overflow-auto rounded-md border border-border bg-subtle p-2 text-xs whitespace-pre-wrap">{t.body}</pre>
      </Section>
      <Section title="Danger zone">
        <ConfirmDialog
          trigger={<Button variant="danger" size="sm" className="justify-self-start">Delete template</Button>}
          title={`Delete ${t.key}?`}
          description="Emails for this key go back to the built-in text."
          confirmLabel="Delete"
          destructive
          onConfirm={async () => {
            await ok(api.DELETE("/api/v1/email-templates/{id}", { params: { path: { id: t.id } } }));
            await refresh();
          }}
        />
      </Section>
    </div>
  ),
  empty: { title: "No custom templates", description: "Emails use the built-in text until you add a template for a key." },
};

export default function Page() {
  return <ResourceView config={config} />;
}
