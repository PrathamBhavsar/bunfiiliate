"use client";

import { useQuery } from "@tanstack/react-query";
import QRCode from "qrcode";
import { useEffect, useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { ok } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";
import { queryClient } from "@/lib/query";
import { forget, recall, remember } from "@/lib/revealed";

// Not under "me": refresh() leaves the ["me"] session query alone by key prefix.
const statusKey = ["security", "totp"] as const;
const REVEAL_KIND = "totp-recovery";
const REVEAL_ID = "self";

function QrImage({ uri }: { uri: string }) {
  const [src, setSrc] = useState<string>();
  useEffect(() => {
    let live = true;
    QRCode.toDataURL(uri, { margin: 1, width: 192 }).then((url) => live && setSrc(url));
    return () => {
      live = false;
    };
  }, [uri]);
  // eslint-disable-next-line @next/next/no-img-element
  return src ? <img src={src} alt="QR code for your authenticator app" width={192} height={192} className="rounded-md bg-white" /> : null;
}

/** Recovery codes are shown once: held in memory until the user confirms they saved them. */
function RecoveryCodes({ codes, onDone }: { codes: string[]; onDone: () => void }) {
  const [saved, setSaved] = useState(false);
  return (
    <div className="grid gap-2" data-testid="recovery-codes">
      <p className="text-xs text-muted">Save these recovery codes now. Each works once, and they are never shown again.</p>
      <ul className="grid grid-cols-2 gap-1 rounded-md border border-border bg-subtle p-2 font-mono text-xs">
        {codes.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
      <label className="flex items-center gap-2 text-xs">
        <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />
        I saved them
      </label>
      <div className="flex justify-end">
        <Button size="sm" variant="primary" disabled={!saved} onClick={onDone}>
          Done
        </Button>
      </div>
    </div>
  );
}

function Message({ result }: { result: { ok: boolean; text: string } | null }) {
  if (!result) return null;
  return (
    <p role={result.ok ? "status" : "alert"} className={result.ok ? "text-xs text-success" : "text-xs text-danger"}>
      {result.text}
    </p>
  );
}

function PasswordAndCode({ submitLabel, pending, onSubmit }: { submitLabel: string; pending: boolean; onSubmit: (v: { current_password: string; code: string }) => void }) {
  function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const f = new FormData(e.currentTarget);
    onSubmit({ current_password: String(f.get("current_password")), code: String(f.get("code")).trim() });
    e.currentTarget.reset();
  }
  return (
    <form onSubmit={submit} className="grid gap-2">
      <div className="grid gap-1">
        <Label htmlFor="sec-password">Current password</Label>
        <Input id="sec-password" name="current_password" type="password" autoComplete="current-password" required />
      </div>
      <div className="grid gap-1">
        <Label htmlFor="sec-code">Authenticator code</Label>
        <Input id="sec-code" name="code" inputMode="numeric" autoComplete="one-time-code" required />
      </div>
      <div className="flex justify-end">
        <Button type="submit" size="sm" variant="primary" disabled={pending}>
          {pending ? "Working…" : submitLabel}
        </Button>
      </div>
    </form>
  );
}

export function SecuritySection() {
  const status = useQuery({
    queryKey: statusKey,
    queryFn: ({ signal }) => ok(api.GET("/api/v1/auth/totp/status", { signal })),
  });
  const [enrolment, setEnrolment] = useState<{ otpauth_uri: string; secret_base32: string } | null>(null);
  const [codes, setCodes] = useState<string[] | null>(() => recall(REVEAL_KIND, REVEAL_ID)?.split(" ") ?? null);
  const [mode, setMode] = useState<"regenerate" | "disable" | null>(null);
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const reload = () => queryClient.invalidateQueries({ queryKey: statusKey });

  async function run(fn: () => Promise<void>) {
    setPending(true);
    setResult(null);
    try {
      await fn();
    } catch (err) {
      setResult({ ok: false, text: errorMessage(err) });
    } finally {
      setPending(false);
    }
  }

  const reveal = (list: string[]) => {
    remember(REVEAL_KIND, REVEAL_ID, list.join(" "));
    setCodes(list);
  };
  const dismiss = () => {
    forget(REVEAL_KIND, REVEAL_ID);
    setCodes(null);
  };

  const start = () =>
    run(async () => {
      setEnrolment(await ok(api.POST("/api/v1/auth/totp/enrol", { body: {} })));
    });

  const confirm = (body: { current_password: string; code: string }) =>
    run(async () => {
      const res = await ok(api.POST("/api/v1/auth/totp/confirm", { body }));
      setEnrolment(null);
      reveal(res.recovery_codes);
      await reload();
    });

  const regenerate = (body: { current_password: string; code: string }) =>
    run(async () => {
      const res = await ok(api.POST("/api/v1/auth/totp/recovery-codes/regenerate", { body }));
      setMode(null);
      reveal(res.recovery_codes);
      await reload();
    });

  const disable = (body: { current_password: string; code: string }) =>
    run(async () => {
      await ok(api.POST("/api/v1/auth/totp/disable", { body }));
      setMode(null);
      setResult({ ok: true, text: "Two-step sign-in is off." });
      await reload();
    });

  return (
    <section aria-labelledby="security-title" className="grid gap-3 rounded-lg border border-border bg-surface p-3">
      <div className="grid gap-1">
        <h2 id="security-title" className="text-sm font-semibold">
          Security
        </h2>
        <p className="text-xs text-muted">Two-step sign-in with an authenticator app. It also lets you reset your own password.</p>
      </div>
      {status.isPending ? (
        <p className="text-xs text-muted">Loading…</p>
      ) : status.isError ? (
        <p role="alert" className="text-xs text-danger">
          {errorMessage(status.error)}
        </p>
      ) : codes ? (
        <RecoveryCodes codes={codes} onDone={dismiss} />
      ) : enrolment ? (
        <div className="grid gap-2">
          <p className="text-xs text-muted">Scan the code, or enter the key by hand. Then confirm with a code from the app.</p>
          <QrImage uri={enrolment.otpauth_uri} />
          <code className="break-all text-xs" data-testid="totp-secret">
            {enrolment.secret_base32}
          </code>
          <PasswordAndCode submitLabel="Confirm and enable" pending={pending} onSubmit={confirm} />
        </div>
      ) : status.data.enrolled ? (
        <div className="grid gap-2">
          <p className="text-xs" role="status">
            Two-step sign-in is <strong>on</strong>. {status.data.recovery_codes_remaining} recovery codes left.
          </p>
          {mode ? (
            <PasswordAndCode submitLabel={mode === "disable" ? "Turn off" : "Regenerate codes"} pending={pending} onSubmit={mode === "disable" ? disable : regenerate} />
          ) : (
            <div className="flex flex-wrap gap-2">
              <Button size="sm" onClick={() => setMode("regenerate")}>
                Regenerate recovery codes
              </Button>
              <Button size="sm" variant="danger" onClick={() => setMode("disable")}>
                Turn off
              </Button>
            </div>
          )}
          {mode && (
            <div className="flex justify-end">
              <Button size="sm" onClick={() => setMode(null)}>
                Cancel
              </Button>
            </div>
          )}
        </div>
      ) : (
        <div className="flex items-center gap-2">
          <p className="text-xs">Two-step sign-in is <strong>off</strong>.</p>
          <Button size="sm" variant="primary" disabled={pending} onClick={start}>
            Set up
          </Button>
        </div>
      )}
      <Message result={result} />
    </section>
  );
}
