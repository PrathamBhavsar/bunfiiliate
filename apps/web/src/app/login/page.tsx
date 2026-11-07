"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, type FormEvent } from "react";
import { BootScreen } from "@/components/boot-screen";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { errorMessage } from "@/lib/api";
import { roleOf, useLogin, useMe } from "@/lib/auth";
import { homeFor } from "@/lib/nav";

export default function LoginPage() {
  const me = useMe();
  const login = useLogin();
  const router = useRouter();
  const user = me.data;

  useEffect(() => {
    if (user) router.replace(homeFor(roleOf(user)));
  }, [user, router]);

  if (me.isPending || user) return <BootScreen />;

  function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = new FormData(e.currentTarget);
    login.mutate({ email: String(form.get("email")).trim(), password: String(form.get("password")) });
  }

  return (
    <main className="grid min-h-dvh place-items-center p-4">
      <form onSubmit={submit} className="grid w-full max-w-xs gap-4 rounded-lg border border-border bg-surface p-5">
        <div className="grid gap-1">
          <span aria-hidden className="size-3 rounded-sm bg-accent" />
          <h1 className="text-base font-semibold">Sign in to bunfiiliate</h1>
          <p className="text-xs text-muted">Use your admin or affiliate account.</p>
        </div>
        <div className="grid gap-1">
          <Label htmlFor="email">Email</Label>
          <Input id="email" name="email" type="email" autoComplete="username" required autoFocus />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="password">Password</Label>
          <Input id="password" name="password" type="password" autoComplete="current-password" required />
        </div>
        {login.isError && (
          <p role="alert" className="text-xs text-danger">
            {errorMessage(login.error)}
          </p>
        )}
        <Button type="submit" variant="primary" disabled={login.isPending}>
          {login.isPending ? "Signing in…" : "Sign in"}
        </Button>
        <p className="text-xs text-muted">
          <Link href="/forgot-password" className="underline">Forgot password?</Link>
        </p>
        <p className="text-xs text-muted">
          New affiliate? <Link href="/register" className="underline">Create an account</Link>
        </p>
      </form>
    </main>
  );
}
