import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";

import { cn } from "@/lib/utils";

/**
 * 首次扫描本地会话的可感知进度提示。
 *
 * 背景：没有缓存时 `/sessions` 要读完全部会话才返回（本机约 20–30 秒），
 * 期间只有骨架屏，用户无法判断是「在扫」还是「卡住了」。这里给一条会走秒的
 * 状态条，并说明预期耗时——**不改变扫描耗时本身**（长期聚合缓存是 T2 的非目标）。
 *
 * 无障碍：`role="status"` + `aria-live="polite"`，读屏会在内容变化时播报；
 * 计时精度 1 秒，组件卸载即清理，不产生后台定时器。
 */
export function ScanProgressHint({
  label = "正在扫描本地会话",
  detail = "首次启动需读取全部会话，通常 20–30 秒",
  className,
}: {
  label?: string;
  detail?: string;
  className?: string;
}) {
  const [seconds, setSeconds] = useState(0);

  useEffect(() => {
    const timer = window.setInterval(() => setSeconds((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, []);

  return (
    <div
      role="status"
      aria-live="polite"
      className={cn(
        "flex flex-wrap items-center gap-x-2 gap-y-1 rounded-[9px] border border-primary-border bg-primary-soft px-3 py-2 text-xs text-primary-soft-foreground",
        className,
      )}
    >
      <Loader2 className="size-3.5 shrink-0 animate-spin" aria-hidden />
      <span className="min-w-0">
        {label}…
        {/* 秒数每秒变化，加 aria-hidden 避免读屏每秒播报一次；标签本身已说明在扫描。 */}
        <span aria-hidden className="ml-1 tabular-nums">
          已等待 {seconds} 秒
        </span>
      </span>
      {detail ? <span className="text-muted-foreground sm:ml-auto">{detail}</span> : null}
    </div>
  );
}
