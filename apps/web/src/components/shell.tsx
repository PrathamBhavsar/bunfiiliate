"use client";

import { LogOut, Menu } from "lucide-react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { useState, type ReactNode } from "react";
import type { User } from "@/lib/api";
import { roleOf, useLogout } from "@/lib/auth";
import { navFor } from "@/lib/nav";
import { cn } from "@/lib/utils";
import { ThemeToggle } from "./theme-toggle";
import { Button } from "./ui/button";
import { Dialog, DialogDescription, DialogTitle, DialogTrigger, SheetContent } from "./ui/dialog";

function Nav({ user, onNavigate }: { user: User; onNavigate?: () => void }) {
  const pathname = usePathname();
  const items = navFor(roleOf(user));
  const groups = [...new Set(items.map((i) => i.group))];
  return (
    <nav aria-label="Main" className="grid gap-4">
      {groups.map((group) => (
        <div key={group} className="grid gap-0.5">
          <p className="px-2 pb-1 text-[11px] font-medium tracking-wide text-muted uppercase">{group}</p>
          {items
            .filter((i) => i.group === group)
            .map(({ href, label, icon: Icon }) => {
              const active = pathname.replace(/\/$/, "") === href;
              return (
                <Link
                  key={href}
                  href={href}
                  onClick={onNavigate}
                  aria-current={active ? "page" : undefined}
                  className={cn(
                    "flex h-8 items-center gap-2 rounded-md px-2 text-[13px] hover:bg-subtle",
                    active && "bg-subtle font-medium shadow-[inset_2px_0_0_var(--accent)]",
                  )}
                >
                  <Icon aria-hidden className={cn("size-4", active ? "text-accent-text" : "text-muted")} />
                  {label}
                </Link>
              );
            })}
        </div>
      ))}
    </nav>
  );
}

const Brand = () => (
  <span className="flex items-center gap-2 text-sm font-semibold">
    <span aria-hidden className="size-2.5 rounded-sm bg-accent" />
    bunfiiliate
  </span>
);

export function Shell({ user, children }: { user: User; children: ReactNode }) {
  const router = useRouter();
  const logout = useLogout();
  const [menuOpen, setMenuOpen] = useState(false);

  return (
    <div className="min-h-dvh md:grid md:grid-cols-[14rem_1fr]">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-accent focus:px-3 focus:py-1.5 focus:text-accent-foreground"
      >
        Skip to content
      </a>
      <aside className="sticky top-0 hidden h-dvh flex-col gap-4 overflow-y-auto border-r border-border bg-surface p-3 md:flex">
        <div className="px-2 py-1">
          <Brand />
        </div>
        <Nav user={user} />
      </aside>

      <div className="flex min-w-0 flex-col">
        <header className="sticky top-0 z-30 flex h-11 items-center gap-2 border-b border-border bg-surface/90 px-3 backdrop-blur">
          <Dialog open={menuOpen} onOpenChange={setMenuOpen}>
            <DialogTrigger asChild>
              <Button variant="ghost" size="icon" aria-label="Open menu" className="md:hidden">
                <Menu />
              </Button>
            </DialogTrigger>
            <SheetContent side="left">
              <DialogTitle>
                <Brand />
              </DialogTitle>
              <DialogDescription className="sr-only">Navigation</DialogDescription>
              <div className="min-h-0 flex-1 overflow-y-auto">
                <Nav user={user} onNavigate={() => setMenuOpen(false)} />
              </div>
            </SheetContent>
          </Dialog>
          <span className="md:hidden">
            <Brand />
          </span>
          <div className="ml-auto flex items-center gap-1">
            <span className="hidden max-w-48 truncate text-xs text-muted sm:block">
              {user.email} · {roleOf(user).replace("_", " ")}
            </span>
            <ThemeToggle />
            <Button
              variant="ghost"
              size="icon"
              aria-label="Sign out"
              disabled={logout.isPending}
              onClick={() => logout.mutate(undefined, { onSettled: () => router.replace("/login") })}
            >
              <LogOut />
            </Button>
          </div>
        </header>
        <main id="main" tabIndex={-1} className="min-w-0 flex-1 p-3 sm:p-4">
          {children}
        </main>
      </div>
    </div>
  );
}
