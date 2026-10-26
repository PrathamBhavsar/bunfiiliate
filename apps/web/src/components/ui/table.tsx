import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

export function Table({ className, ...props }: ComponentProps<"table">) {
  return (
    <div className="overflow-x-auto">
      <table className={cn("w-full min-w-max border-collapse text-left", className)} {...props} />
    </div>
  );
}

export const Th = ({ className, ...props }: ComponentProps<"th">) => (
  <th
    scope="col"
    className={cn("h-8 border-b border-border px-3 text-xs font-medium whitespace-nowrap text-muted", className)}
    {...props}
  />
);

export const Td = ({ className, ...props }: ComponentProps<"td">) => (
  <td className={cn("h-9 border-b border-border px-3 whitespace-nowrap", className)} {...props} />
);
