"use client";

import Link from "next/link";
import { useMutation } from "@tanstack/react-query";
import { type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { api, errorMessage } from "@/lib/api";

export default function ForgotPasswordPage() {
  const reset = useMutation({
    mutationFn: async (body: { email: string; code: string; new_password: string }) => {
      const { response, error } = await api.POST("/api/v1/auth/password/reset", { body });
      if (!response.ok) throw new Error(errorMessage(error));
    },
  });

  function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = new FormData(e.currentTarget);
    reset.mutate({
      email: String(form.get("email")).trim(),
      code: String(form.get("code")).trim(),
      new_password: String(form.get("new_password")),
    });
  }

  return (
    <main className="grid min-h-dvh place-items-center p-4">
      <form onSubmit={submit} className="grid w-full max-w-xs gap-4 rounded-lg border border-border bg-surface p-5">
        <div className="grid gap-1">
          <h1 className="text-base font-semibold">Reset your password</h1>
          <p className="text-xs text-muted">
            Enter an authenticator code or a recovery code. Accounts without two-step sign-in need an admin to reset.
          </p>
        </div>
        <div className="grid gap-1">
          <Label htmlFor="email">Email</Label>
          <Input id="email" name="email" type="email" autoComplete="username" required autoFocus />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="code">Authenticator or recovery code</Label>
          <Input id="code" name="code" autoComplete="one-time-code" required />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="new_password">New password (12+ characters)</Label>
          <Input id="new_password" name="new_password" type="password" autoComplete="new-password" minLength={12} maxLength={256} required />
        </div>
        {reset.isError && (
          <p role="alert" className="text-xs text-danger">
            {reset.error.message}
          </p>
        )}
        {reset.isSuccess && (
          <p role="status" className="text-xs">
            Password changed. All sessions were signed out.
          </p>
        )}
        <Button type="submit" variant="primary" disabled={reset.isPending}>
          {reset.isPending ? "Resetting…" : "Reset password"}
        </Button>
        <p className="text-xs text-muted">
          <Link href="/login" className="underline">Back to sign in</Link>
        </p>
      </form>
    </main>
  );
}
