"use client";

import { keepPreviousData, useQuery } from "@tanstack/react-query";

export type Page<T> = { items: T[]; total: number };
export type ListParams = {
  page: number;
  pageSize: number;
  search: string;
  filters: Record<string, string>;
};
export type ListFn<T, P extends object = ListParams> = (params: P, signal: AbortSignal) => Promise<Page<T>>;

// Query keys need a stable identity for a list function; module-level functions keep one.
const ids = new WeakMap<object, number>();
let nextId = 0;
export function idOf(fn: object) {
  let id = ids.get(fn);
  if (id === undefined) ids.set(fn, (id = ++nextId));
  return id;
}

/** Paged list query. Keeps the previous page on screen while the next one loads. */
export function useResource<T, P extends object = ListParams>(listFn: ListFn<T, P>, params: P) {
  const q = useQuery({
    queryKey: ["resource", idOf(listFn), params],
    queryFn: ({ signal }) => listFn(params, signal),
    placeholderData: keepPreviousData,
  });
  return {
    items: q.data?.items ?? [],
    total: q.data?.total ?? 0,
    isLoading: q.isPending,
    isFetching: q.isFetching,
    error: q.error,
    refetch: q.refetch,
  };
}
