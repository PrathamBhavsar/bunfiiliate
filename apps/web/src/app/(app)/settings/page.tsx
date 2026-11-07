"use client";

import { useQuery } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";
import { useState, type FormEvent } from "react";
import { SecuritySection } from "@/components/security-section";
import { Button } from "@/components/ui/button";
import { Label, Textarea } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { ok, refresh } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";

type Settings = Record<string, unknown>;

function Editor({ settings }: { settings: Settings }) {
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setResult(null);
    let body: Settings;
    try {
      const parsed: unknown = JSON.parse(String(new FormData(e.currentTarget).get("json")));
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) throw new Error("Settings must be a JSON object");
      body = parsed as Settings;
    } catch (err) {
      return setResult({ ok: false, text: err instanceof SyntaxError ? `Not valid JSON: ${err.message}` : errorMessage(err) });
    }
    setPending(true);
    try {
      // The spec types an open object as all-undefined; the cast satisfies it.
      await ok(api.PUT("/api/v1/settings", { body: body as Record<string, undefined> }));
      await refresh();
      setResult({ ok: true, text: "Settings saved." });
    } catch (err) {
      setResult({ ok: false, text: errorMessage(err) });
    } finally {
      setPending(false);
    }
  }

  return (
    <form onSubmit={submit} className="grid gap-3 rounded-lg border border-border bg-surface p-3">
      <div className="grid gap-1">
        <Label htmlFor="settings-json">Workspace settings (JSON object)</Label>
        <Textarea id="settings-json" name="json" rows={14} spellCheck={false} className="font-mono text-xs" defaultValue={JSON.stringify(settings, null, 2)} />
        <p className="text-xs text-muted">Saving replaces the whole object. Limit 16 KiB. The audit log records which keys were set, not their values.</p>
      </div>
      {result && (
        <p role={result.ok ? "status" : "alert"} className={result.ok ? "text-xs text-success" : "text-xs text-danger"}>
          {result.text}
        </p>
      )}
      <div className="flex justify-end">
        <Button type="submit" variant="primary" disabled={pending}>
          {pending ? "Saving…" : "Save settings"}
        </Button>
      </div>
    </form>
  );
}

export default function Page() {
  const q = useQuery({
    queryKey: ["settings"],
    queryFn: ({ signal }) => ok(api.GET("/api/v1/settings", { signal })) as Promise<Settings>,
  });
  return (
    <section aria-labelledby="page-title" className="grid max-w-2xl gap-3">
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          Settings
        </h1>
        <p className="text-xs text-muted">Free-form configuration for this workspace.</p>
      </header>
      <SecuritySection />
      {q.isPending ? (
        <Skeleton className="h-64" />
      ) : q.isError ? (
        <div role="alert" className="grid justify-items-center gap-2 rounded-lg border border-border bg-surface px-4 py-12 text-center">
          <TriangleAlert aria-hidden className="size-5 text-danger" />
          <p className="font-medium">Could not load settings</p>
          <p className="text-xs text-muted">{errorMessage(q.error)}</p>
          <Button size="sm" onClick={() => q.refetch()}>
            Retry
          </Button>
        </div>
      ) : (
        <Editor key={JSON.stringify(q.data)} settings={q.data} />
      )}
    </section>
  );
}
