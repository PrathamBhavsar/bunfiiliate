"use client";

import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, ChevronRight, Search } from "lucide-react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useState, type ReactNode } from "react";
import { idOf, useResource, type ListFn, type ListParams } from "@/lib/use-resource";
import { type Column, DataTable } from "./data-table";
import { Button } from "./ui/button";
import { Dialog, DialogDescription, DialogTitle, SheetContent } from "./ui/dialog";
import { Input, Select } from "./ui/input";
import { Skeleton } from "./ui/skeleton";
import { EmptyState, ErrorState } from "./state";

/** A select filter, or a `date` filter whose value is `YYYY-MM-DD` (empty = unset). */
export type FilterDef = { id: string; label: string } & (
  | { type?: "select"; options: { value: string; label: string }[] }
  | { type: "date" }
);

export type ResourceConfig<T extends { id: string }> = {
  title: string;
  description?: string;
  /** Marks a stub page. The value names the task that wires real data, e.g. "T31". */
  placeholder?: string;
  columns: Column<T>[];
  list: ListFn<T>;
  /** Fetch one row by id, for a `?id=` link to a row that is not on the current page. */
  get?: (id: string, signal: AbortSignal) => Promise<T>;
  filters?: FilterDef[];
  searchPlaceholder?: string;
  pageSize?: number;
  /** Header buttons, e.g. a `FormDialog` trigger. */
  actions?: ReactNode;
  detailTitle?: (row: T) => string;
  detail?: (row: T) => ReactNode;
  empty: { title: string; description: string };
  /** Primary action in the empty state; when the header action does not already say it. */
  emptyAction?: ReactNode;
};

/** A list page from one config: filters, table, pagination, detail sheet (`?id=`), and every state. */
export function ResourceView<T extends { id: string }>({
  config,
  headingLevel = 1,
}: {
  config: ResourceConfig<T>;
  /** 2 when the page already has its own h1 (a second list on the page). */
  headingLevel?: 1 | 2;
}) {
  const Heading = headingLevel === 1 ? "h1" : "h2";
  const headingId = headingLevel === 1 ? "page-title" : `${config.title.toLowerCase().replace(/\W+/g, "-")}-title`;
  const pageSize = config.pageSize ?? 25;
  const [params, setParams] = useState<ListParams>({ page: 1, pageSize, search: "", filters: {} });
  const { items, total, isLoading, isFetching, error, refetch } = useResource(config.list, params);

  const router = useRouter();
  const pathname = usePathname();
  const selectedId = useSearchParams().get("id");
  const select = (id: string | null) =>
    router.replace(id ? `${pathname}?id=${encodeURIComponent(id)}` : pathname, { scroll: false });

  const change = (patch: Partial<ListParams>) => setParams((p) => ({ ...p, ...patch, page: 1 }));
  const lastPage = Math.max(1, Math.ceil(total / pageSize));
  const from = total === 0 ? 0 : (params.page - 1) * pageSize + 1;
  const to = Math.min(total, params.page * pageSize);
  const hasControls = Boolean(config.searchPlaceholder || config.filters?.length);

  return (
    <section aria-labelledby={headingId} className="grid gap-3">
      <header className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <Heading id={headingId} className="text-base font-semibold">
            {config.title}
          </Heading>
          {config.description && <p className="text-xs text-muted">{config.description}</p>}
        </div>
        {config.actions}
      </header>

      {hasControls && (
        <div className="flex flex-wrap items-center gap-2">
          {config.searchPlaceholder && (
            <form
              role="search"
              className="relative w-full sm:w-64"
              onSubmit={(e) => {
                e.preventDefault();
                change({ search: String(new FormData(e.currentTarget).get("q") ?? "").trim() });
              }}
            >
              <Search aria-hidden className="pointer-events-none absolute top-2 left-2.5 size-4 text-muted" />
              <Input
                name="q"
                type="search"
                aria-label={config.searchPlaceholder}
                placeholder={config.searchPlaceholder}
                className="pl-8"
                onChange={(e) => e.target.value === "" && change({ search: "" })}
              />
            </form>
          )}
          {config.filters?.map((f) =>
            f.type === "date" ? (
              <label key={f.id} className="flex items-center gap-1.5 text-xs text-muted">
                {f.label}
                <Input
                  type="date"
                  className="w-auto text-foreground"
                  value={params.filters[f.id] ?? ""}
                  onChange={(e) => change({ filters: { ...params.filters, [f.id]: e.target.value } })}
                />
              </label>
            ) : (
              <Select
                key={f.id}
                aria-label={f.label}
                className="w-auto"
                value={params.filters[f.id] ?? ""}
                onChange={(e) => change({ filters: { ...params.filters, [f.id]: e.target.value } })}
              >
                <option value="">{f.label}: all</option>
                {f.options.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </Select>
            ),
          )}
        </div>
      )}

      <div className="overflow-hidden rounded-lg border border-border bg-surface">
        {error && !items.length ? (
          <ErrorState error={error} what={config.title.toLowerCase()} onRetry={refetch} />
        ) : !isLoading && items.length === 0 ? (
          <EmptyState
            title={config.empty.title}
            description={config.empty.description}
            action={
              config.placeholder ? (
                <span className="rounded border border-dashed border-border px-1.5 py-0.5 text-xs text-muted">
                  Placeholder · lands in {config.placeholder}
                </span>
              ) : (
                config.emptyAction
              )
            }
          />
        ) : (
          <DataTable<T>
            label={config.title}
            columns={config.columns}
            rows={items}
            loading={isLoading}
            selectedId={selectedId}
            onRowClick={config.detail && ((row) => select(row.id))}
          />
        )}
      </div>

      {total > 0 && (
        <nav aria-label="Pagination" className="flex items-center justify-between text-xs text-muted">
          <span aria-live="polite">
            {from}–{to} of {total}
            {isFetching && " · updating"}
          </span>
          <div className="flex gap-1">
            <Button
              size="icon"
              aria-label="Previous page"
              disabled={params.page <= 1}
              onClick={() => setParams((p) => ({ ...p, page: p.page - 1 }))}
            >
              <ChevronLeft />
            </Button>
            <Button
              size="icon"
              aria-label="Next page"
              disabled={params.page >= lastPage}
              onClick={() => setParams((p) => ({ ...p, page: p.page + 1 }))}
            >
              <ChevronRight />
            </Button>
          </div>
        </nav>
      )}

      {config.detail && selectedId && (
        <DetailSheet config={config} id={selectedId} items={items} onClose={() => select(null)} />
      )}
    </section>
  );
}

function DetailSheet<T extends { id: string }>({
  config,
  id,
  items,
  onClose,
}: {
  config: ResourceConfig<T>;
  id: string;
  items: T[];
  onClose: () => void;
}) {
  const local = items.find((r) => r.id === id);
  const remote = useQuery({
    queryKey: ["resource-detail", idOf(config.list), id],
    queryFn: ({ signal }) => config.get!(id, signal),
    enabled: !local && Boolean(config.get),
  });
  const row = local ?? remote.data;

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <SheetContent>
        <DialogTitle>{row && config.detailTitle ? config.detailTitle(row) : config.title}</DialogTitle>
        <DialogDescription className="sr-only">Details for the selected row</DialogDescription>
        <div className="min-h-0 flex-1 overflow-y-auto">
          {row ? (
            config.detail?.(row)
          ) : remote.isPending && config.get ? (
            <div className="grid gap-2">
              <Skeleton className="h-4 w-1/2" />
              <Skeleton className="h-4 w-3/4" />
            </div>
          ) : (
            <p className="text-xs text-muted">Not found.</p>
          )}
        </div>
      </SheetContent>
    </Dialog>
  );
}
