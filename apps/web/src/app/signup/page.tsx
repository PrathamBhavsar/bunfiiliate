"use client";

import { useMutation } from "@tanstack/react-query";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { ok } from "@/lib/admin";
import { api, errorMessage, meKey } from "@/lib/api";
import { roleOf } from "@/lib/auth";
import { homeFor } from "@/lib/nav";
import { clearUserData, queryClient } from "@/lib/query";

export default function SignupPage() {
  const router = useRouter();
  const [mismatch, setMismatch] = useState(false);
  const signup = useMutation({
    mutationFn: (body: { workspace_name: string; admin_email: string; admin_password: string }) =>
      ok(api.POST("/api/v1/signup", { body })),
    onSuccess: (user) => {
      clearUserData(meKey);
      queryClient.setQueryData(meKey, user);
      router.replace(homeFor(roleOf(user)));
    },
  });

  function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = new FormData(e.currentTarget);
    const password = String(form.get("password"));
    if (password !== String(form.get("confirm"))) {
      setMismatch(true);
      return;
    }
    setMismatch(false);
    signup.mutate({
      workspace_name: String(form.get("workspace_name")).trim(),
      admin_email: String(form.get("email")).trim(),
      admin_password: password,
    });
  }

  return (
    <main className="grid min-h-dvh place-items-center p-4">
      <form onSubmit={submit} className="grid w-full max-w-xs gap-4 rounded-lg border border-border bg-surface p-5">
        <div className="grid gap-1">
          <span aria-hidden className="size-3 rounded-sm bg-accent" />
          <h1 className="text-base font-semibold">Create a workspace</h1>
          <p className="text-xs text-muted">You become its admin.</p>
        </div>
        <div className="grid gap-1">
          <Label htmlFor="workspace_name">Workspace name</Label>
          <Input id="workspace_name" name="workspace_name" required autoFocus />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="email">Email</Label>
          <Input id="email" name="email" type="email" autoComplete="username" required />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="password">Password</Label>
          <Input id="password" name="password" type="password" autoComplete="new-password" minLength={12} required />
          <p className="text-xs text-muted">At least 12 characters.</p>
        </div>
        <div className="grid gap-1">
          <Label htmlFor="confirm">Confirm password</Label>
          <Input id="confirm" name="confirm" type="password" autoComplete="new-password" minLength={12} required />
        </div>
        {(mismatch || signup.isError) && (
          <p role="alert" className="text-xs text-danger">
            {mismatch ? "Passwords do not match." : errorMessage(signup.error)}
          </p>
        )}
        <Button type="submit" variant="primary" disabled={signup.isPending}>
          {signup.isPending ? "Creating…" : "Create workspace"}
        </Button>
        <p className="text-xs text-muted">
          Have an account? <Link href="/login" className="underline">Sign in</Link>
        </p>
      </form>
    </main>
  );
}
