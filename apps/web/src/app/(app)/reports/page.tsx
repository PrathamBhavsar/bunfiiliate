"use client";

import { Download } from "lucide-react";
import { useMemo, useState, type FormEvent } from "react";
import { ScopeSelects } from "@/components/analytics-filters";
import { Button } from "@/components/ui/button";
import { Input, Label, Select } from "@/components/ui/input";
import { DIMS, downloadReport, lastDays, type Dim, type Scope } from "@/lib/analytics";
import { errorMessage } from "@/lib/api";

export default function Page() {
  const initial = useMemo(() => lastDays(30), []);
  const [scope, setScope] = useState<Scope>({ program_id: "", affiliate_id: "" });
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const f = new FormData(e.currentTarget);
    const range = { from: String(f.get("from")), to: String(f.get("to")) };
    if (range.from > range.to) return setResult({ ok: false, text: "From must be on or before To." });
    setPending(true);
    setResult(null);
    try {
      const name = await downloadReport(range, scope, String(f.get("dim")) as Dim);
      setResult({ ok: true, text: `Downloaded ${name}` });
    } catch (err) {
      setResult({ ok: false, text: errorMessage(err) });
    } finally {
      setPending(false);
    }
  }

  return (
    <section aria-labelledby="page-title" className="grid max-w-xl gap-3">
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          Reports
        </h1>
        <p className="text-xs text-muted">
          Download daily analytics as CSV: day, dimension, clicks, conversions, revenue and commission in cents, and
          conversion rate in basis points. Ranges are UTC days, at most 366.
        </p>
      </header>
      <form onSubmit={submit} className="grid gap-3 rounded-lg border border-border bg-surface p-3">
        <div className="grid grid-cols-2 gap-3">
          <div className="grid gap-1">
            <Label htmlFor="report-from">From</Label>
            <Input id="report-from" name="from" type="date" required defaultValue={initial.from} />
          </div>
          <div className="grid gap-1">
            <Label htmlFor="report-to">To</Label>
            <Input id="report-to" name="to" type="date" required defaultValue={initial.to} />
          </div>
        </div>
        <div className="grid gap-1">
          <Label htmlFor="report-dim">Dimension</Label>
          <Select id="report-dim" name="dim" defaultValue="total">
            {DIMS.map((d) => (
              <option key={d.value} value={d.value}>
                {d.label}
              </option>
            ))}
          </Select>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <ScopeSelects scope={scope} onChange={setScope} />
        </div>
        {result && (
          <p role={result.ok ? "status" : "alert"} className={result.ok ? "text-xs text-success" : "text-xs text-danger"}>
            {result.text}
          </p>
        )}
        <div className="flex justify-end">
          <Button type="submit" variant="primary" disabled={pending}>
            <Download /> {pending ? "Preparing…" : "Download CSV"}
          </Button>
        </div>
      </form>
    </section>
  );
}
