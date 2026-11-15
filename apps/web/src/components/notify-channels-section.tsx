"use client";

import { useQuery } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { ok } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";
import { queryClient } from "@/lib/query";

// Not under "me": refresh() leaves the ["me"] session query alone by key prefix.
const key = ["notify-channels"] as const;
type Channel = "telegram" | "discord";
type Result = { ok: boolean; text: string } | null;

const CHANNELS: { id: Channel; label: string; field: "telegram_url" | "discord_url"; set: "telegram_set" | "discord_set" }[] = [
  { id: "telegram", label: "Telegram", field: "telegram_url", set: "telegram_set" },
  { id: "discord", label: "Discord", field: "discord_url", set: "discord_set" },
];

/** Telegram and Discord webhook URLs. The server never returns them, so the fields start empty. */
export function NotifyChannelsSection() {
  const q = useQuery({ queryKey: key, queryFn: ({ signal }) => ok(api.GET("/api/v1/settings/notify-channels", { signal })) });
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<Result>(null);

  async function run(fn: () => Promise<string>) {
    setPending(true);
    setResult(null);
    try {
      setResult({ ok: true, text: await fn() });
    } catch (err) {
      setResult({ ok: false, text: errorMessage(err) });
    } finally {
      setPending(false);
    }
  }

  function save(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;
    const f = new FormData(form);
    // Blank means "leave as is"; clearing is its own button.
    const body: { telegram_url?: string; discord_url?: string } = {};
    for (const c of CHANNELS) {
      const v = String(f.get(c.field) ?? "").trim();
      if (v) body[c.field] = v;
    }
    void run(async () => {
      if (!Object.keys(body).length) throw new Error("Enter a URL to save");
      await ok(api.PUT("/api/v1/settings/notify-channels", { body }));
      form.reset();
      await queryClient.invalidateQueries({ queryKey: key });
      return "Channels saved.";
    });
  }

  const clear = (c: (typeof CHANNELS)[number]) =>
    run(async () => {
      await ok(api.PUT("/api/v1/settings/notify-channels", { body: { [c.field]: null } }));
      await queryClient.invalidateQueries({ queryKey: key });
      return `${c.label} cleared.`;
    });

  const test = (c: (typeof CHANNELS)[number]) =>
    run(async () => {
      await ok(api.POST("/api/v1/settings/test-notify-channel", { body: { channel: c.id } }));
      return `Test message sent to ${c.label}.`;
    });

  return (
    <form onSubmit={save} aria-labelledby="notify-title" className="grid gap-3 rounded-lg border border-border bg-surface p-3">
      <div>
        <h2 id="notify-title" className="text-sm font-semibold">
          Notification channels
        </h2>
        <p className="text-xs text-muted">Payouts, reversals and payout requests post a short message here. URLs are stored encrypted and never shown again.</p>
      </div>
      {CHANNELS.map((c) => {
        const set = q.data?.[c.set] ?? false;
        return (
          <div key={c.id} className="grid gap-1">
            <Label htmlFor={`notify-${c.id}`}>
              {c.label} webhook URL {set && <span className="text-success">(set)</span>}
            </Label>
            <div className="flex flex-wrap items-center gap-2">
              <Input id={`notify-${c.id}`} name={c.field} type="url" autoComplete="off" placeholder={set ? "Enter a new URL to replace" : "https://"} className="min-w-0 flex-1" />
              <Button type="button" size="sm" disabled={pending || !set} onClick={() => test(c)}>
                Send test message
              </Button>
              <Button type="button" size="sm" disabled={pending || !set} onClick={() => clear(c)}>
                Clear
              </Button>
            </div>
          </div>
        );
      })}
      {q.isError && (
        <p role="alert" className="text-xs text-danger">
          {errorMessage(q.error)}
        </p>
      )}
      {result && (
        <p role={result.ok ? "status" : "alert"} className={result.ok ? "text-xs text-success" : "text-xs text-danger"}>
          {result.text}
        </p>
      )}
      <div className="flex justify-end">
        <Button type="submit" variant="primary" disabled={pending}>
          {pending ? "Working…" : "Save channels"}
        </Button>
      </div>
    </form>
  );
}
