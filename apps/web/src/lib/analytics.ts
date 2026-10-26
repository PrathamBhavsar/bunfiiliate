"use client";

import { type components } from "@bunfiiliate/sdk";
import { useQuery } from "@tanstack/react-query";
import { ok, type Program } from "./admin";
import { api } from "./api";

export type Summary = components["schemas"]["Summary"];
export type Point = components["schemas"]["Point"];
export type Dim = components["schemas"]["Dim"];
export type Range = { from: string; to: string };
/** Empty string = all. */
export type Scope = { program_id: string; affiliate_id: string };

export const DIMS: { value: Dim; label: string }[] = [
  { value: "country", label: "Country" },
  { value: "device", label: "Device" },
  { value: "os", label: "OS" },
  { value: "browser", label: "Browser" },
  { value: "referrer", label: "Referrer" },
  { value: "total", label: "Total" },
];

/** The last `n` UTC days, ending today (the API's day boundary). */
export function lastDays(n: number): Range {
  const now = new Date();
  const day = (offset: number) => new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() - offset)).toISOString().slice(0, 10);
  return { from: day(n - 1), to: day(0) };
}

const query = (r: Range, s: Scope) => ({ from: r.from, to: r.to, program_id: s.program_id || undefined, affiliate_id: s.affiliate_id || undefined });

export function useSummary(range: Range, scope: Scope, dim: Dim | null) {
  return useQuery({
    queryKey: ["analytics", "summary", range, scope, dim],
    queryFn: ({ signal }) =>
      ok(api.GET("/api/v1/analytics/summary", { signal, params: { query: { ...query(range, scope), dim: dim ?? undefined } } })),
  });
}

export function useTimeseries(range: Range, scope: Scope) {
  return useQuery({
    queryKey: ["analytics", "timeseries", range, scope],
    queryFn: ({ signal }) =>
      ok(api.GET("/api/v1/analytics/timeseries", { signal, params: { query: query(range, scope) } })),
  });
}

/**
 * Currency to label money with: the chosen program's, or the shared one when every program uses the
 * same. null = mixed (or programs still loading): cents from different currencies must not be added.
 */
export function currencyFor(programs: Program[] | undefined, programId: string): string | null {
  if (!programs) return null;
  const set = new Set((programId ? programs.filter((p) => p.id === programId) : programs).map((p) => p.currency));
  return set.size === 0 ? "" : set.size === 1 ? ([...set][0] ?? "") : null;
}

/** Fetch the analytics CSV and hand it to the browser as a file. Returns the file name. */
export async function downloadReport(range: Range, scope: Scope, dim: Dim) {
  const blob = await ok<Blob>(
    api.GET("/api/v1/reports/analytics.csv", {
      parseAs: "blob",
      params: { query: { ...query(range, scope), dim: dim ?? undefined } },
    }) as never,
  );
  const name = `analytics-${range.from}-${range.to}-${dim}.csv`;
  const url = URL.createObjectURL(blob);
  const a = Object.assign(document.createElement("a"), { href: url, download: name });
  a.click();
  URL.revokeObjectURL(url);
  return name;
}
