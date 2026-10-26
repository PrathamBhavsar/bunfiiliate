"use client";

import { useState, type ComponentProps } from "react";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/api";

type Props = Omit<ComponentProps<typeof Button>, "onClick"> & { onAction: () => Promise<unknown> };

/** A button for a one-shot API mutation: disabled while it runs, and a failure is shown next to it instead of being swallowed. */
export function ActionButton({ onAction, disabled, children, ...props }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <>
      <Button
        {...props}
        disabled={disabled || busy}
        onClick={async () => {
          setBusy(true);
          setError(null);
          try {
            await onAction();
          } catch (e) {
            setError(errorMessage(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {children}
      </Button>
      {error && (
        <span role="alert" className="text-xs text-danger">
          {error}
        </span>
      )}
    </>
  );
}
