import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

/*
 * 按钮颜色规则（T1 深海蓝）：
 * - `default` 实色主色 + `--primary-hover` 悬停（不再用 `bg-primary/90` 降透明）；
 * - `destructive` 实心危险底 + `--destructive-foreground`（不再写 text-white）；
 * - `brand` 是**唯一**允许渐变的变体，只给 hero CTA（同屏最多一个）。
 * 圆角统一 9px（含 sm/lg/icon 的覆盖），普通卡片 14px、弹窗 12px。
 *
 * 禁用态：实心变体（default / destructive / brand）不再用 `opacity-50` 压暗——
 * 那会让白字与底色一起褪成浅灰（实测约 1.5:1，几乎看不清）。改为中性底 +
 * `--muted-foreground` 文字（深/浅均 ≥4.3:1），渐变用 `bg-none` 去掉；
 * 描边/次/幽灵/链接变体本身是浅底，保留 `opacity-50` 即可。
 */
const buttonVariants = cva(
  "inline-flex cursor-pointer items-center justify-center gap-2 whitespace-nowrap rounded-[9px] text-sm font-medium transition-all disabled:pointer-events-none disabled:shadow-none [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4 shrink-0 [&_svg]:shrink-0 outline-none focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px] aria-invalid:ring-destructive/20 aria-invalid:border-destructive",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow-xs hover:bg-primary-hover disabled:bg-muted disabled:text-muted-foreground",
        destructive:
          "bg-destructive text-destructive-foreground shadow-xs hover:bg-destructive/90 focus-visible:ring-destructive/20 disabled:bg-muted disabled:text-muted-foreground",
        brand:
          "bg-[linear-gradient(180deg,var(--primary-gradient-from),var(--primary-gradient-to))] text-white shadow-[var(--shadow-brand),inset_0_1px_0_var(--brand-highlight)] hover:brightness-105 active:brightness-95 disabled:bg-none disabled:bg-muted disabled:text-muted-foreground disabled:brightness-100",
        outline:
          "border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground disabled:opacity-50",
        secondary:
          "bg-secondary text-secondary-foreground shadow-xs hover:bg-secondary/80 disabled:opacity-50",
        ghost: "hover:bg-accent hover:text-accent-foreground disabled:opacity-50",
        link: "text-primary underline-offset-4 hover:underline disabled:opacity-50",
      },
      size: {
        default: "h-9 px-4 py-2 has-[>svg]:px-3",
        sm: "h-8 rounded-[9px] gap-1.5 px-3 has-[>svg]:px-2.5",
        lg: "h-10 rounded-[9px] px-6 has-[>svg]:px-4",
        icon: "size-9",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

function Button({
  className,
  variant,
  size,
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean;
  }) {
  const Comp = asChild ? Slot : "button";
  return (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  );
}

export { Button, buttonVariants };
