"use client";

import { usePathname, useRouter } from "next/navigation";
import { Suspense, useEffect, type ReactNode } from "react";
import { BootScreen } from "@/components/boot-screen";
import { Shell } from "@/components/shell";
import { Skeleton } from "@/components/ui/skeleton";
import { roleOf, useMe } from "@/lib/auth";
import { canAccess, homeFor } from "@/lib/nav";

/** Session and role guard for every signed-in route. Client-side only: the API enforces roles for real. */
export default function AppLayout({ children }: { children: ReactNode }) {
  const me = useMe();
  const router = useRouter();
  const pathname = usePathname();
  const user = me.data;
  const role = user ? roleOf(user) : null;
  const allowed = role !== null && canAccess(role, pathname);

  useEffect(() => {
    if (me.isPending || me.isError) return;
    if (!role) router.replace("/login");
    else if (!allowed) router.replace(homeFor(role));
  }, [me.isPending, me.isError, role, allowed, router]);

  if (me.isError) return <BootScreen error onRetry={() => me.refetch()} />;
  if (!user || !allowed) return <BootScreen />;

  return (
    <Shell user={user}>
      <Suspense fallback={<Skeleton className="h-24" />}>{children}</Suspense>
    </Shell>
  );
}
