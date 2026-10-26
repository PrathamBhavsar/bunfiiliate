"use client";

import { type components } from "@bunfiiliate/sdk";
import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { api, errorMessage, meKey } from "./api";
import { queryClient } from "./query";
import type { Page } from "./use-resource";

export type Program = components["schemas"]["Program"];
export type Plan = components["schemas"]["Plan"];
export type Affiliate = components["schemas"]["Affiliate"];
export type ReferralCode = components["schemas"]["ReferralCode"];
export type Conversion = components["schemas"]["Conversion"];
export type ConversionStatus = components["schemas"]["ConversionStatus"];
export type Customer = components["schemas"]["Customer"];
export type Account = components["schemas"]["Account"];
export type Entry = components["schemas"]["Entry"];
export type Balance = components["schemas"]["Balance"];
export type Batch = components["schemas"]["BatchView"];
export type BatchDetail = components["schemas"]["BatchDetail"];

/** Largest page the API serves. Dropdowns read one page of it. */
export const MAX_LIMIT = 200;

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
      (await ok(api.GET("/api/v1/programs", { signal, params: { query: { page: 1, limit: MAX_LIMIT } } }))).items,
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
            params: { query: { page: 1, limit: MAX_LIMIT } },
          }),
        )
      ).items,
  });
}

/** Program and affiliate names for filters and id columns. Falls back to the raw id while loading. */
export function useLookups() {
  const programs = usePrograms().data;
  const affiliates = useAffiliates().data;
  return useMemo(
    () => ({
      programOptions: (programs ?? []).map((p) => ({ value: p.id, label: p.name })),
      affiliateOptions: (affiliates ?? []).map((a) => ({ value: a.id, label: a.email })),
      programName: (id: string) => programs?.find((p) => p.id === id)?.name ?? id,
      affiliateEmail: (id: string) => affiliates?.find((a) => a.id === id)?.email ?? id,
    }),
    [programs, affiliates],
  );
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/** Integer cents as `12.34 USD`. Display only; all math stays in integers. */
export function formatCents(cents: number, currency: string) {
  const abs = Math.abs(cents);
  return `${cents < 0 ? "-" : ""}${Math.floor(abs / 100)}.${pad2(abs % 100)} ${currency}`.trimEnd();
}

/** An API timestamp in the viewer's locale; the raw string if it does not parse. */
export function formatTime(iso: string) {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
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
