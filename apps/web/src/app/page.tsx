"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { BootScreen } from "@/components/boot-screen";
import { roleOf, useMe } from "@/lib/auth";
import { homeFor } from "@/lib/nav";

export default function Index() {
  const me = useMe();
  const router = useRouter();
  const user = me.data;

  useEffect(() => {
    if (me.isPending || me.isError) return;
    router.replace(user ? homeFor(roleOf(user)) : "/login");
  }, [me.isPending, me.isError, user, router]);

  return <BootScreen error={me.isError} onRetry={() => me.refetch()} />;
}
