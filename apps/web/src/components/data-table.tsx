import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Skeleton } from "./ui/skeleton";
import { Table, Td, Th } from "./ui/table";

export type Column<T> = {
  id: string;
  header: string;
  cell: (row: T) => ReactNode;
  align?: "right";
  className?: string;
};

const SKELETON_ROWS = 6;

/** Dense table. Rows with `onRowClick` are keyboard reachable (Tab, then Enter or Space). */
export function DataTable<T extends { id: string }>({
  columns,
  rows,
  loading,
  label,
  selectedId,
  onRowClick,
}: {
  columns: Column<T>[];
  rows: T[];
  loading?: boolean;
  /** Accessible name of the table. */
  label: string;
  selectedId?: string | null;
  onRowClick?: (row: T) => void;
}) {
  return (
    <Table aria-busy={loading} aria-label={label}>
      <thead>
        <tr>
          {columns.map((c) => (
            <Th key={c.id} className={cn(c.align === "right" && "text-right")}>
              {c.header}
            </Th>
          ))}
        </tr>
      </thead>
      <tbody>
        {loading
          ? Array.from({ length: SKELETON_ROWS }, (_, i) => (
              <tr key={i}>
                {columns.map((c) => (
                  <Td key={c.id}>
                    <Skeleton className="h-3.5 w-3/4" />
                  </Td>
                ))}
              </tr>
            ))
          : rows.map((row) => (
              <tr
                key={row.id}
                tabIndex={onRowClick ? 0 : undefined}
                aria-selected={onRowClick ? row.id === selectedId : undefined}
                onClick={onRowClick && (() => onRowClick(row))}
                onKeyDown={
                  onRowClick &&
                  ((e) => {
                    if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
                      e.preventDefault();
                      onRowClick(row);
                    }
                  })
                }
                className={cn(
                  onRowClick && "cursor-pointer hover:bg-subtle",
                  row.id === selectedId && "bg-subtle",
                )}
              >
                {columns.map((c) => (
                  <Td key={c.id} className={cn(c.align === "right" && "text-right tabular-nums", c.className)}>
                    {c.cell(row)}
                  </Td>
                ))}
              </tr>
            ))}
      </tbody>
    </Table>
  );
}
