"use client";

import { type components } from "@bunfiiliate/sdk";
import { useQuery } from "@tanstack/react-query";
import { api, errorMessage, meKey } from "./api";
import { queryClient } from "./query";
import type { Page } from "./use-resource";

export type Program = components["schemas"]["Program"];
export type Plan = components["schemas"]["Plan"];
export type Affiliate = components["schemas"]["Affiliate"];
export type ReferralCode = components["schemas"]["ReferralCode"];

/** Largest page the API serves. Dropdowns read one page of it. */
export const MAX_LIMIT = 200;

/**
 * The spec lists `page`, `limit` and `status` as path params (utoipa default), so the typed client
 * has no query slot. Send them as both: path keys without a `{name}` in the URL are ignored, and
 * the cast hides `query` from the (wrong) generated type.
 */
export function listParams<P extends object, Q extends object>(path: P, query: Q) {
  return { path: { ...path, ...query }, query } as unknown as { path: P & Q; query?: undefined };
}

/** Unwrap an SDK call: throw the API's error message on a non-2xx, return the body (undefined on 204). */
export async function ok<T>(call: Promise<{ data?: T; error?: unknown; response: Response }>): Promise<T> {
  const { data, error, response } = await call;
  if (!response.ok) throw new Error(errorMessage(error));
  return data as T;
}

export const toPage = <T>({ items, total }: { items: T[]; total: number }): Page<T> => ({ items, total });

/** Refetch every list and detail after a mutation. The session query is left alone. */
export function refresh() {
  return queryClient.invalidateQueries({ predicate: (q) => q.queryKey[0] !== meKey[0] });
}

export function usePrograms() {
  return useQuery({
    queryKey: ["programs", "options"],
    queryFn: async ({ signal }) =>
      (await ok(api.GET("/api/v1/programs", { signal, params: listParams({}, { page: 1, limit: MAX_LIMIT }) }))).items,
  });
}

export function useAffiliates() {
  return useQuery({
    queryKey: ["affiliates", "options"],
    queryFn: async ({ signal }) =>
      (
        await ok(
          api.GET("/api/v1/affiliates", {
            signal,
            params: listParams({}, { page: 1, limit: MAX_LIMIT, status: null }),
          }),
        )
      ).items,
  });
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/** Integer cents as `12.34 USD`. Display only; all math stays in integers. */
export function formatCents(cents: number, currency: string) {
  const abs = Math.abs(cents);
  return `${cents < 0 ? "-" : ""}${Math.floor(abs / 100)}.${pad2(abs % 100)} ${currency}`.trimEnd();
}

/** Basis points as a percent: 1250 -> `12.50%`. */
export const formatBps = (bps: number) => `${Math.floor(bps / 100)}.${pad2(bps % 100)}%`;

const INT_MAX = 2_147_483_647;

/** Parse a form value as a whole number in `min..=max`; throws a field-named message otherwise. */
export function wholeNumber(raw: string, label: string, min: number, max = INT_MAX) {
  const v = raw.trim();
  if (!/^\d+$/.test(v)) throw new Error(`${label} must be a whole number`);
  const n = Number(v);
  if (n < min || n > max) throw new Error(`${label} must be between ${min} and ${max}`);
  return n;
}

export const optionalNumber = (raw: string, label: string, min: number, max = INT_MAX) =>
  raw.trim() === "" ? undefined : wholeNumber(raw, label, min, max);

export function required(raw: string, label: string) {
  const v = raw.trim();
  if (!v) throw new Error(`${label} is required`);
  return v;
}

/** RFC 3339 timestamp as `2026-03-07 14:05 UTC`. */
export const formatTime = (iso: string) => `${iso.slice(0, 16).replace("T", " ")} UTC`;
