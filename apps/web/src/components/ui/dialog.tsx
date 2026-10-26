"use client";

import { X } from "lucide-react";
import { Dialog as Primitive } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

export const Dialog = Primitive.Root;
export const DialogTrigger = Primitive.Trigger;
export const DialogClose = Primitive.Close;
export const DialogTitle = ({ className, ...props }: ComponentProps<typeof Primitive.Title>) => (
  <Primitive.Title className={cn("text-sm font-semibold", className)} {...props} />
);
export const DialogDescription = ({ className, ...props }: ComponentProps<typeof Primitive.Description>) => (
  <Primitive.Description className={cn("text-xs text-muted", className)} {...props} />
);

const overlay = "fixed inset-0 z-40 bg-black/50 data-[state=open]:animate-fade-in";
const closeButton =
  "absolute top-2.5 right-2.5 inline-flex size-7 items-center justify-center rounded-md hover:bg-subtle";

export function DialogContent({ className, children, ...props }: ComponentProps<typeof Primitive.Content>) {
  return (
    <Primitive.Portal>
      <Primitive.Overlay className={overlay} />
      <Primitive.Content
        className={cn(
          "fixed top-1/2 left-1/2 z-50 grid w-[calc(100vw-2rem)] max-w-md -translate-x-1/2 -translate-y-1/2 gap-4 rounded-lg border border-border bg-surface p-4 shadow-lg data-[state=open]:animate-fade-in",
          className,
        )}
        {...props}
      >
        {children}
        <Primitive.Close aria-label="Close" className={closeButton}>
          <X className="size-4" />
        </Primitive.Close>
      </Primitive.Content>
    </Primitive.Portal>
  );
}

export function SheetContent({
  side = "right",
  className,
  children,
  ...props
}: ComponentProps<typeof Primitive.Content> & { side?: "left" | "right" }) {
  return (
    <Primitive.Portal>
      <Primitive.Overlay className={overlay} />
      <Primitive.Content
        className={cn(
          "fixed inset-y-0 z-50 flex w-[min(24rem,100vw)] flex-col gap-3 border-border bg-surface p-4 shadow-lg",
          side === "right"
            ? "right-0 border-l data-[state=open]:animate-slide-in-right"
            : "left-0 border-r data-[state=open]:animate-slide-in-left",
          className,
        )}
        {...props}
      >
        {children}
        <Primitive.Close aria-label="Close" className={closeButton}>
          <X className="size-4" />
        </Primitive.Close>
      </Primitive.Content>
    </Primitive.Portal>
  );
}
