import { TriangleAlert } from "lucide-react";
import { Button } from "./ui/button";
import { Skeleton } from "./ui/skeleton";

/** Full-page state while the session loads, or when the API cannot be reached. */
export function BootScreen({ error, onRetry }: { error?: boolean; onRetry?: () => void }) {
  return (
    <div className="grid min-h-dvh place-items-center p-4">
      {error ? (
        <div role="alert" className="grid justify-items-center gap-2 text-center">
          <TriangleAlert aria-hidden className="size-5 text-danger" />
          <p className="font-medium">Cannot reach the API</p>
          <p className="text-xs text-muted">Check that the server is running, then retry.</p>
          <Button size="sm" onClick={onRetry}>
            Retry
          </Button>
        </div>
      ) : (
        <div role="status" aria-label="Loading" className="grid w-48 gap-2">
          <Skeleton className="h-4" />
          <Skeleton className="h-4 w-2/3" />
        </div>
      )}
    </div>
  );
}
