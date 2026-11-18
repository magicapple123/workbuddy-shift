import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

/*
 * Alert 的语义变体全部走 `--danger*` / `--warning*` / `--success*` token：
 * 浅底 + 描边 + `*-foreground` 文字，**不再**用 `/90`、`/80`、`dark:text-*` 压暗凑色。
 * 说明文字（AlertDescription）在语义变体下由父级用 `*:data-[slot=alert-description]`
 * 覆盖成同一个 foreground token，保证浅底上的对比度一致（默认变体仍是 muted）。
 */
const alertVariants = cva(
  "relative w-full rounded-lg border px-4 py-3 text-sm grid has-[>svg]:grid-cols-[calc(var(--spacing)*4)_1fr] grid-cols-[0_1fr] has-[>svg]:gap-x-3 gap-y-0.5 items-start [&>svg]:size-4 [&>svg]:translate-y-0.5",
  {
    variants: {
      variant: {
        default: "bg-card text-card-foreground",
        destructive:
          "border-danger-border bg-danger-soft text-danger-foreground [&>svg]:text-danger *:data-[slot=alert-description]:text-danger-foreground",
        warning:
          "border-warning-border bg-warning-soft text-warning-foreground [&>svg]:text-warning *:data-[slot=alert-description]:text-warning-foreground",
        success:
          "border-success-border bg-success-soft text-success-foreground [&>svg]:text-success *:data-[slot=alert-description]:text-success-foreground",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  },
);

function Alert({
  className,
  variant,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof alertVariants>) {
  return (
    <div
      data-slot="alert"
      role="alert"
      className={cn(alertVariants({ variant }), className)}
      {...props}
    />
  );
}

function AlertTitle({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-title"
      className={cn("col-start-2 line-clamp-1 min-h-4 font-medium tracking-tight", className)}
      {...props}
    />
  );
}

function AlertDescription({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-description"
      className={cn(
        "text-muted-foreground col-start-2 grid justify-items-start gap-1 text-sm [&_p]:leading-relaxed",
        className,
      )}
      {...props}
    />
  );
}

export { Alert, AlertTitle, AlertDescription };
