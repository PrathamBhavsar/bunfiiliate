import { Inbox, Lock, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { errorMessage, isForbidden } from "@/lib/api";
import { Button } from "./ui/button";
import { Skeleton } from "./ui/skeleton";

/** Nothing to show yet: say why, and offer the primary action when there is one. */
export function EmptyState({ title, description, action }: { title: string; description: string; action?: ReactNode }) {
  return (
    <div data-testid="empty-state" className="grid justify-items-center gap-1 px-4 py-12 text-center">
      <Inbox aria-hidden className="size-5 text-muted" />
      <p className="font-medium">{title}</p>
      <p className="max-w-sm text-xs text-muted">{description}</p>
      {action && <div className="mt-1">{action}</div>}
    </div>
  );
}

/** A failed load. Shows the server's message (never raw JSON) with Retry; a 403 is "No access", not an error to retry. */
export function ErrorState({ error, what, onRetry }: { error: unknown; what: string; onRetry: () => unknown }) {
  if (isForbidden(error)) {
    return (
      <div role="alert" data-testid="forbidden-state" className="grid justify-items-center gap-1 px-4 py-12 text-center">
        <Lock aria-hidden className="size-5 text-muted" />
        <p className="font-medium">No access</p>
        <p className="max-w-sm text-xs text-muted">Your role cannot view {what}. Ask a workspace admin if you need it.</p>
      </div>
    );
  }
  return (
    <div role="alert" data-testid="error-state" className="grid justify-items-center gap-2 px-4 py-12 text-center">
      <TriangleAlert aria-hidden className="size-5 text-danger" />
      <p className="font-medium">Could not load {what}</p>
      <p className="text-xs text-muted">{errorMessage(error)}</p>
      <Button size="sm" onClick={() => onRetry()}>
        Retry
      </Button>
    </div>
  );
}

/** Skeleton rows for a block that is still loading. */
export function LoadingRows({ rows = 3 }: { rows?: number }) {
  return (
    <div role="status" aria-label="Loading" className="grid gap-2 p-3">
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} className="h-4 w-full" />
      ))}
    </div>
  );
}
