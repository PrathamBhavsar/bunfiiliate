"use client";

import { useMutation } from "@tanstack/react-query";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Suspense, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { ok } from "@/lib/admin";
import { api, errorMessage } from "@/lib/api";

function RegisterForm() {
  const workspace = useSearchParams().get("workspace") ?? "";
  const register = useMutation({
    mutationFn: (body: { workspace: string; email: string; password: string; payout_email?: string }) =>
      ok(api.POST("/api/v1/affiliates/register", { body })),
  });

  function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = new FormData(e.currentTarget);
    const payout = String(form.get("payout_email")).trim();
    register.mutate({
      workspace: String(form.get("workspace")).trim(),
      email: String(form.get("email")).trim(),
      password: String(form.get("password")),
      payout_email: payout || undefined,
    });
  }

  if (register.isSuccess) {
    return (
      <div className="grid w-full max-w-xs gap-4 rounded-lg border border-border bg-surface p-5">
        <h1 className="text-base font-semibold">Account created</h1>
        <p role="status" className="text-xs text-muted">
          Your affiliate account is {register.data.status}. {register.data.status === "pending" && "An admin must approve it before you can create links. "}
          You can sign in now.
        </p>
        <Link href="/login" className="inline-flex h-8 items-center justify-center rounded-md bg-accent px-3 text-[13px] font-medium text-accent-foreground">
          Go to sign in
        </Link>
      </div>
    );
  }

  return (
    <form onSubmit={submit} className="grid w-full max-w-xs gap-4 rounded-lg border border-border bg-surface p-5">
      <div className="grid gap-1">
        <span aria-hidden className="size-3 rounded-sm bg-accent" />
        <h1 className="text-base font-semibold">Become an affiliate</h1>
        <p className="text-xs text-muted">Create an account to get referral links and earn commission.</p>
      </div>
      <div className="grid gap-1">
        <Label htmlFor="workspace">Workspace</Label>
        <Input id="workspace" name="workspace" required defaultValue={workspace} placeholder="acme" autoCapitalize="none" />
      </div>
      <div className="grid gap-1">
        <Label htmlFor="email">Email</Label>
        <Input id="email" name="email" type="email" autoComplete="username" required />
      </div>
      <div className="grid gap-1">
        <Label htmlFor="password">Password</Label>
        <Input id="password" name="password" type="password" autoComplete="new-password" minLength={8} required />
        <p className="text-xs text-muted">At least 8 characters.</p>
      </div>
      <div className="grid gap-1">
        <Label htmlFor="payout_email">Payout email (optional)</Label>
        <Input id="payout_email" name="payout_email" type="email" />
      </div>
      {register.isError && (
        <p role="alert" className="text-xs text-danger">
          {errorMessage(register.error)}
        </p>
      )}
      <Button type="submit" variant="primary" disabled={register.isPending}>
        {register.isPending ? "Creating…" : "Create account"}
      </Button>
      <p className="text-xs text-muted">
        Already have an account? <Link href="/login" className="underline">Sign in</Link>
      </p>
    </form>
  );
}

export default function RegisterPage() {
  return (
    <main className="grid min-h-dvh place-items-center p-4">
      <Suspense>
        <RegisterForm />
      </Suspense>
    </main>
  );
}
