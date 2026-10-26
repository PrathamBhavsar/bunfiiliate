"use client";

import type { UseQueryResult } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { errorMessage } from "@/lib/api";
import { Button } from "./ui/button";
import { Skeleton } from "./ui/skeleton";

/** Titled block inside a detail sheet, with an optional action (e.g. an add button) on the right. */
export function Section({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section className="grid gap-2 border-t border-border pt-3 first:border-t-0 first:pt-0">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-xs font-semibold">{title}</h3>
        {action}
      </div>
      {children}
    </section>
  );
}

/** Loading, error and empty states for a sub-list. `children` renders the `<li>` rows. */
export function ListState<T>({
  query,
  empty,
  children,
}: {
  query: UseQueryResult<T[]>;
  empty: string;
  children: (items: T[]) => ReactNode;
}) {
  if (query.isPending) return <Skeleton className="h-8" />;
  if (query.isError) {
    return (
      <div role="alert" className="flex items-center justify-between gap-2 text-xs text-danger">
        <span>{errorMessage(query.error)}</span>
        <Button size="sm" onClick={() => query.refetch()}>
          Retry
        </Button>
      </div>
    );
  }
  if (!query.data.length) return <p className="text-xs text-muted">{empty}</p>;
  return <ul className="divide-y divide-border rounded-md border border-border">{children(query.data)}</ul>;
}

/** Label/value pairs. */
export function Facts({ rows }: { rows: [label: string, value: ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
      {rows.map(([label, value]) => (
        <div key={label} className="contents">
          <dt className="text-muted">{label}</dt>
          <dd className="min-w-0 break-words">{value}</dd>
        </div>
      ))}
    </dl>
  );
}
