"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { ok } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";
import { forget, recall, remember } from "@/lib/revealed";

const KIND = "reset-token";

/** Issue a one-hour password reset token. The API returns it once; it lives in this tab's memory until dismissed. */
export function ResetTokenDialog({ userId, email }: { userId: string; email: string }) {
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState<string | undefined>(() => recall(KIND, userId));
  const [expires, setExpires] = useState<string>();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function issue() {
    setPending(true);
    setError(null);
    try {
      const res = await ok(api.POST("/api/v1/users/{id}/password-reset-token", { params: { path: { id: userId } } }));
      remember(KIND, userId, res.token);
      setToken(res.token);
      setExpires(res.expires_at);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPending(false);
    }
  }

  function close() {
    forget(KIND, userId);
    setToken(undefined);
    setExpires(undefined);
    setError(null);
    setOpen(false);
  }

  return (
    <Dialog open={open} onOpenChange={(next) => (next ? setOpen(true) : close())}>
      <DialogTrigger asChild>
        <Button size="sm">Issue reset token</Button>
      </DialogTrigger>
      <DialogContent>
        <div className="grid gap-1">
          <DialogTitle>Reset token for {email}</DialogTitle>
          <DialogDescription>
            {token
              ? "Copy it now and send it to the user. It is never shown again."
              : "Creates a one-time token that expires in an hour and replaces any earlier one. The user is notified and the action is audited."}
          </DialogDescription>
        </div>
        {token && (
          <div className="grid gap-1">
            <code data-testid="reset-token" className="break-all rounded-md border border-border bg-subtle p-2 text-xs">
              {token}
            </code>
            {expires && <p className="text-xs text-muted">Expires {new Date(expires).toLocaleString()}. Page: /reset-password</p>}
          </div>
        )}
        {error && (
          <p role="alert" className="text-xs text-danger">
            {error}
          </p>
        )}
        <div className="flex justify-end gap-2">
          {token ? (
            <Button variant="primary" onClick={close}>
              I copied it
            </Button>
          ) : (
            <>
              <DialogClose asChild>
                <Button>Cancel</Button>
              </DialogClose>
              <Button variant="primary" disabled={pending} onClick={issue}>
                {pending ? "Working…" : "Issue token"}
              </Button>
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
