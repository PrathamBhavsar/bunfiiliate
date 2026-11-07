"use client";

import { useQuery } from "@tanstack/react-query";
import { Eye, EyeOff, Plus, Trash2 } from "lucide-react";
import { usePathname, useRouter } from "next/navigation";
import { useState } from "react";
import {
  type Plan,
  type Program,
  formatBps,
  formatCents,
  MAX_LIMIT,
  ok,
  optionalNumber,
  refresh,
  required,
  useAffiliates,
  wholeNumber,
} from "@/lib/admin";
import { api } from "@/lib/api";
import { recall, remember } from "@/lib/revealed";
import { Facts, ListState, Section } from "./admin-ui";
import { ConfirmDialog } from "./confirm-dialog";
import { FormDialog, type Field } from "./form-dialog";
import { Button } from "./ui/button";

const DURATIONS = { one_month: "One month", x_months: "X months", lifetime: "Lifetime" } as const;

const MODELS = [
  { value: "last_touch", label: "Last touch" },
  { value: "first_touch", label: "First touch" },
];

const YES_NO = [
  { value: "false", label: "No" },
  { value: "true", label: "Yes" },
];

export function programFields(opts: { create?: boolean; plans?: Plan[] }): Field[] {
  return [
    { name: "name", label: "Name", required: true },
    ...(opts.create ? [{ name: "currency", label: "Currency", required: true, placeholder: "USD", hint: "3-letter ISO code. Cannot change later." }] : []),
    { name: "attribution_model", label: "Attribution", type: "select", options: MODELS },
    { name: "cookie_days", label: "Cookie days", type: "number", required: true, hint: "At least 1." },
    { name: "hold_days", label: "Hold days", type: "number", required: true, hint: "Days before a conversion auto-approves. 0 or more." },
    { name: "auto_approve", label: "Auto-approve conversions", type: "select", options: YES_NO },
    ...(opts.plans?.length
      ? [
          {
            name: "default_plan_id",
            label: "Default plan",
            type: "select" as const,
            options: [{ value: "", label: "Keep current" }, ...opts.plans.map((p) => ({ value: p.id, label: planLabel(p, "") }))],
          },
        ]
      : []),
  ];
}

/** Parse the program form. Throws a field-named message that `FormDialog` shows. */
export function programBody(v: Record<string, string>) {
  return {
    name: required(v.name ?? "", "Name"),
    attribution_model: v.attribution_model === "first_touch" ? ("first_touch" as const) : ("last_touch" as const),
    cookie_days: wholeNumber(v.cookie_days ?? "", "Cookie days", 1),
    hold_days: wholeNumber(v.hold_days ?? "", "Hold days", 0),
    auto_approve: v.auto_approve === "true",
  };
}

export function planLabel(plan: Plan, currency: string) {
  const amount =
    plan.kind === "percent" ? formatBps(plan.rate_bps ?? 0) : formatCents(plan.amount_cents ?? 0, currency);
  const duration = plan.duration === "x_months" ? `${plan.months} months` : DURATIONS[plan.duration as keyof typeof DURATIONS];
  return `${amount} · ${duration?.toLowerCase() ?? plan.duration}`;
}

export function ProgramDetail({ program }: { program: Program }) {
  const router = useRouter();
  const pathname = usePathname();
  const plans = usePlans(program.id);

  return (
    <div className="grid gap-4">
      <Section
        title="Settings"
        action={
          <FormDialog
            trigger={<Button size="sm">Edit</Button>}
            title="Edit program"
            fields={programFields({ plans: plans.data })}
            initial={{
              name: program.name,
              attribution_model: program.attribution_model,
              cookie_days: String(program.cookie_days),
              hold_days: String(program.hold_days),
              auto_approve: String(program.auto_approve),
            }}
            onSubmit={async (v) => {
              const body = programBody(v);
              await ok(
                api.PATCH("/api/v1/programs/{id}", {
                  params: { path: { id: program.id } },
                  body: { ...body, ...(v.default_plan_id ? { default_plan_id: v.default_plan_id } : {}) },
                }),
              );
              await refresh();
            }}
          />
        }
      >
        <Facts
          rows={[
            ["Currency", program.currency],
            ["Attribution", program.attribution_model.replace("_", " ")],
            ["Cookie days", String(program.cookie_days)],
            ["Hold days", String(program.hold_days)],
            ["Auto-approve", program.auto_approve ? "Yes" : "No"],
          ]}
        />
      </Section>

      <Secrets program={program} />
      <Sites program={program} />
      <Plans program={program} plans={plans} />
      <Enrollments program={program} plans={plans.data ?? []} />

      <Section title="Danger zone">
        <ConfirmDialog
          trigger={<Button variant="danger" size="sm" className="justify-self-start">Delete program</Button>}
          title={`Delete ${program.name}?`}
          description="Removes the program with its sites, plans, enrollments and referral codes. Programs with conversions cannot be deleted."
          confirmLabel="Delete"
          destructive
          onConfirm={async () => {
            await ok(api.DELETE("/api/v1/programs/{id}", { params: { path: { id: program.id } } }));
            router.replace(pathname);
            await refresh();
          }}
        />
      </Section>
    </div>
  );
}

function Secrets({ program }: { program: Program }) {
  const [shown, setShown] = useState(false);
  // The API shows the webhook secret once (create/rotate); this tab remembers it until reload.
  const webhook = recall("program", program.id);
  return (
    <Section
      title="Keys and secrets"
      action={
        <div className="flex gap-1">
          <Button size="sm" aria-pressed={shown} onClick={() => setShown((s) => !s)}>
            {shown ? <EyeOff /> : <Eye />}
            {shown ? "Hide" : "Reveal"}
          </Button>
          <ConfirmDialog
            trigger={<Button size="sm">Rotate</Button>}
            title="Rotate secrets?"
            description="Issues new secrets right away. Anything still signing with the old webhook secret starts failing verification."
            confirmLabel="Rotate"
            destructive
            onConfirm={async () => {
              const rotated = await ok(api.POST("/api/v1/programs/{id}/secrets/rotate", { params: { path: { id: program.id } } }));
              remember("program", program.id, rotated.webhook_secret);
              await refresh();
            }}
          />
        </div>
      }
    >
      <Facts
        rows={[
          ["Public key", <code key="pk" className="break-all">{program.public_key}</code>],
          [
            "Webhook secret",
            webhook ? (
              <code key="wh" className="break-all">{shown ? webhook : "••••••••••••"}</code>
            ) : (
              <span key="wh" className="text-muted">Shown once on create or rotate. Rotate to get a new one.</span>
            ),
          ],
          ["Stripe secret", <span key="st" className="text-muted">Write-only. Set it with Edit.</span>],
          ["Paddle secret", <span key="pd" className="text-muted">Write-only. Set it with Edit.</span>],
        ]}
      />
    </Section>
  );
}

function Sites({ program }: { program: Program }) {
  const sites = useQuery({
    queryKey: ["program", program.id, "sites"],
    queryFn: async ({ signal }) =>
      (
        await ok(
          api.GET("/api/v1/programs/{id}/sites", {
            signal,
            params: { path: { id: program.id }, query: { page: 1, limit: MAX_LIMIT } },
          }),
        )
      ).items,
  });
  return (
    <Section
      title="Sites"
      action={
        <FormDialog
          trigger={
            <Button size="sm">
              <Plus /> Add
            </Button>
          }
          title="Add site"
          description="Referral landing pages must sit on one of these domains."
          fields={[{ name: "domain", label: "Domain", required: true, placeholder: "example.com", hint: "Bare host name, no scheme or path." }]}
          onSubmit={async (v) => {
            const domain = required(v.domain ?? "", "Domain").toLowerCase();
            if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/.test(domain)) {
              throw new Error("Domain must be a bare host name, e.g. example.com");
            }
            await ok(api.POST("/api/v1/programs/{id}/sites", { params: { path: { id: program.id } }, body: { domain } }));
            await refresh();
          }}
        />
      }
    >
      <ListState query={sites} empty="No sites yet. Referral codes need at least one.">
        {(items) =>
          items.map((s) => (
            <li key={s.domain} className="flex items-center justify-between gap-2 px-2 py-1.5 text-xs">
              <span className="min-w-0 break-all">{s.domain}</span>
              <ConfirmDialog
                trigger={
                  <Button size="icon" variant="ghost" aria-label={`Remove ${s.domain}`}>
                    <Trash2 />
                  </Button>
                }
                title={`Remove ${s.domain}?`}
                description="Referral codes that land on this domain are deleted with it."
                confirmLabel="Remove"
                destructive
                onConfirm={async () => {
                  await ok(
                    api.DELETE("/api/v1/programs/{id}/sites/{domain}", {
                      params: { path: { id: program.id, domain: s.domain } },
                    }),
                  );
                  await refresh();
                }}
              />
            </li>
          ))
        }
      </ListState>
    </Section>
  );
}

function usePlans(programId: string) {
  return useQuery({
    queryKey: ["program", programId, "plans"],
    queryFn: async ({ signal }) =>
      (
        await ok(
          api.GET("/api/v1/programs/{id}/plans", {
            signal,
            params: { path: { id: programId }, query: { page: 1, limit: MAX_LIMIT } },
          }),
        )
      ).items,
  });
}

function Plans({ program, plans }: { program: Program; plans: ReturnType<typeof usePlans> }) {
  return (
    <Section
      title="Commission plans"
      action={
        <FormDialog
          trigger={
            <Button size="sm">
              <Plus /> Add
            </Button>
          }
          title="Add commission plan"
          description="Percent plans use basis points (100 bps = 1%). Flat plans use integer cents."
          fields={[
            {
              name: "kind",
              label: "Kind",
              type: "select",
              options: [
                { value: "percent", label: "Percent" },
                { value: "flat", label: "Flat" },
              ],
            },
            { name: "rate_bps", label: "Rate (bps)", type: "number", hint: "Percent plans only. 0 to 10000." },
            { name: "amount_cents", label: `Amount (cents, ${program.currency})`, type: "number", hint: "Flat plans only." },
            {
              name: "duration",
              label: "Duration",
              type: "select",
              options: [
                { value: "lifetime", label: "Lifetime" },
                { value: "one_month", label: "One month" },
                { value: "x_months", label: "X months" },
              ],
            },
            { name: "months", label: "Months", type: "number", hint: "X-months plans only. At least 1." },
          ]}
          onSubmit={async (v) => {
            const percent = v.kind !== "flat";
            const rate = optionalNumber(v.rate_bps ?? "", "Rate (bps)", 0, 10_000);
            const amount = optionalNumber(v.amount_cents ?? "", "Amount (cents)", 0, Number.MAX_SAFE_INTEGER);
            if (percent && (rate === undefined || amount !== undefined)) throw new Error("A percent plan takes a rate in bps only");
            if (!percent && (amount === undefined || rate !== undefined)) throw new Error("A flat plan takes an amount in cents only");
            const duration = v.duration === "x_months" ? "x_months" : v.duration === "one_month" ? "one_month" : "lifetime";
            const months = optionalNumber(v.months ?? "", "Months", 1);
            if (duration === "x_months" ? months === undefined : months !== undefined) {
              throw new Error("Months applies to X-months plans only, and they require it");
            }
            await ok(
              api.POST("/api/v1/programs/{id}/plans", {
                params: { path: { id: program.id } },
                body: { kind: percent ? "percent" : "flat", rate_bps: rate, amount_cents: amount, duration, months },
              }),
            );
            await refresh();
          }}
        />
      }
    >
      <ListState query={plans} empty="No plans yet. Conversions earn no commission until one exists.">
        {(items) =>
          items.map((p) => (
            <li key={p.id} className="flex items-center justify-between gap-2 px-2 py-1.5 text-xs">
              <span className="min-w-0">
                {planLabel(p, program.currency)}
                {p.id === program.default_plan_id && (
                  <span className="ml-1.5 rounded border border-border px-1 text-[11px] text-muted">default</span>
                )}
              </span>
              <ConfirmDialog
                trigger={
                  <Button size="icon" variant="ghost" aria-label="Delete plan">
                    <Trash2 />
                  </Button>
                }
                title="Delete plan?"
                description="Plans that are the program default or an affiliate override cannot be deleted."
                confirmLabel="Delete"
                destructive
                onConfirm={async () => {
                  await ok(
                    api.DELETE("/api/v1/programs/{id}/plans/{plan_id}", {
                      params: { path: { id: program.id, plan_id: p.id } },
                    }),
                  );
                  await refresh();
                }}
              />
            </li>
          ))
        }
      </ListState>
    </Section>
  );
}

function Enrollments({ program, plans }: { program: Program; plans: Plan[] }) {
  const affiliates = useAffiliates();
  const enrollments = useQuery({
    queryKey: ["program", program.id, "enrollments"],
    queryFn: async ({ signal }) =>
      (
        await ok(
          api.GET("/api/v1/programs/{id}/enrollments", {
            signal,
            params: { path: { id: program.id }, query: { page: 1, limit: MAX_LIMIT } },
          }),
        )
      ).items,
  });
  const emailOf = (id: string) => affiliates.data?.find((a) => a.id === id)?.email ?? id;
  const active = (affiliates.data ?? []).filter((a) => a.status === "active");

  return (
    <Section
      title="Enrolled affiliates"
      action={
        <FormDialog
          trigger={
            <Button size="sm">
              <Plus /> Enroll
            </Button>
          }
          title="Enroll affiliate"
          description="Affiliates must be enrolled before they can have referral codes."
          fields={[
            {
              name: "affiliate_id",
              label: "Affiliate",
              type: "select",
              required: true,
              options: [{ value: "", label: "Choose…" }, ...active.map((a) => ({ value: a.id, label: a.email }))],
            },
            {
              name: "plan_override_id",
              label: "Plan override",
              type: "select",
              options: [{ value: "", label: "Use program default" }, ...plans.map((p) => ({ value: p.id, label: planLabel(p, program.currency) }))],
            },
          ]}
          onSubmit={async (v) => {
            const affiliate_id = required(v.affiliate_id ?? "", "Affiliate");
            await ok(
              api.POST("/api/v1/programs/{id}/enrollments", {
                params: { path: { id: program.id } },
                body: { affiliate_id, plan_override_id: v.plan_override_id || undefined },
              }),
            );
            await refresh();
          }}
        />
      }
    >
      <ListState query={enrollments} empty="No affiliates enrolled.">
        {(items) =>
          items.map((e) => (
            <li key={e.affiliate_id} className="flex items-center justify-between gap-2 px-2 py-1.5 text-xs">
              <span className="min-w-0 break-all">{emailOf(e.affiliate_id)}</span>
              <ConfirmDialog
                trigger={
                  <Button size="icon" variant="ghost" aria-label={`Unenroll ${emailOf(e.affiliate_id)}`}>
                    <Trash2 />
                  </Button>
                }
                title="Unenroll affiliate?"
                description="Their referral codes for this program stop working."
                confirmLabel="Unenroll"
                destructive
                onConfirm={async () => {
                  await ok(
                    api.DELETE("/api/v1/programs/{id}/enrollments/{affiliate_id}", {
                      params: { path: { id: program.id, affiliate_id: e.affiliate_id } },
                    }),
                  );
                  await refresh();
                }}
              />
            </li>
          ))
        }
      </ListState>
    </Section>
  );
}
