import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

const field =
  "h-8 w-full rounded-md border border-border bg-surface px-2.5 text-[13px] placeholder:text-muted/70 disabled:opacity-50";

export function Input({ className, ...props }: ComponentProps<"input">) {
  return <input className={cn(field, className)} {...props} />;
}

export function Select({ className, ...props }: ComponentProps<"select">) {
  return <select className={cn(field, "pr-6", className)} {...props} />;
}

export function Textarea({ className, ...props }: ComponentProps<"textarea">) {
  return <textarea className={cn(field, "h-auto min-h-20 py-1.5", className)} {...props} />;
}

export function Label({ className, ...props }: ComponentProps<"label">) {
  return <label className={cn("text-xs font-medium", className)} {...props} />;
}
