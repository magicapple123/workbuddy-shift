import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { JetbrainsDiagnosticsPanel } from "@/components/jetbrains-diagnostics";
import { JetbrainsMark } from "@/components/product-marks";
import * as api from "@/lib/api";
import { displayName } from "@/lib/account-display";
import type { AccountMeta, JetbrainsStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 目标账号 */
  account: AccountMeta | null;
  /** JetBrains 状态（提供已装插件的目录列表与运行状态）。 */
  jetbrainsStatus?: JetbrainsStatus | null;
  /** 切换完成后刷新列表 */
  onDone?: () => void;
}

/**
 * 配置目录前缀 → 展示名。**顺序敏感：长前缀必须在前**（`PyCharmCE` 先于 `PyCharm`，
 * `DataSpell` 先于 `DataGrip` 无所谓但 `IntelliJIdea` 不能被 `IdeaIC` 抢走）。
 *
 * 与后端 `PRODUCTS` 表一一对应（16 个产品）：后端加产品时这里也要加，否则弹窗里
 * 只能看到 `AndroidStudio2024.1` 这种目录名，用户不知道自己切的是哪个 IDE。
 */
const PRODUCT_LABELS: ReadonlyArray<readonly [prefix: string, label: string]> = [
  ["IntelliJIdea", "IntelliJ IDEA"],
  ["IdeaIC", "IntelliJ IDEA CE"],
  ["PyCharmCE", "PyCharm CE"],
  ["PyCharm", "PyCharm"],
  ["WebStorm", "WebStorm"],
  ["PhpStorm", "PhpStorm"],
  ["GoLand", "GoLand"],
  ["CLion", "CLion"],
  ["RubyMine", "RubyMine"],
  ["DataGrip", "DataGrip"],
  ["DataSpell", "DataSpell"],
  ["Rider", "Rider"],
  ["Aqua", "Aqua"],
  ["AppCode", "AppCode"],
  ["MPS", "MPS"],
  ["AndroidStudio", "Android Studio"],
];

/**
 * 配置目录名 → 可读标签：`GoLand2025.2` → `GoLand 2025.2`、
 * `AndroidStudio2024.1` → `Android Studio 2024.1`、`PyCharmCE2025.1` → `PyCharm CE 2025.1`。
 *
 * 认不出的前缀原样返回（诊断场景下保留真实目录名比猜一个产品名更有用）。
 */
export function productLabel(configDir: string): string {
  const hit = PRODUCT_LABELS.find(([prefix]) => configDir.startsWith(prefix));
  if (!hit) return configDir;
  const [prefix, label] = hit;
  const suffix = configDir.slice(prefix.length);
  return suffix ? `${label} ${suffix}` : label;
}

/**
 * JetBrains 插件切换弹窗：选择要写入的 IDE（配置目录）后执行切换。
 *
 * 检测到多个装了插件的 IDE 时可只切其中一个；缺省全选（与「一次切换全家生效」
 * 的缺省行为一致）。只装了一个时同样弹窗——把切换范围显式化，不引入第二种交互。
 */
export function JetbrainsSwitchDialog({ open, onOpenChange, account, jetbrainsStatus, onDone }: Props) {
  const targets = useMemo(
    () => (jetbrainsStatus?.targets ?? []).filter((t) => t.pluginInstalled),
    [jetbrainsStatus],
  );
  /** 运行中但映射不到配置目录的实例：后端会拒绝写入，这里提前阻断并说明原因。 */
  const unmapped = jetbrainsStatus?.unmappedRunning ?? [];
  /** 勾选的配置目录名；undefined = 尚未初始化（效果等同全选）。 */
  const [selected, setSelected] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  /** 上次切换的错误原文（含「已识别 N 个 IDE」与回滚说明）：留在弹窗里可复制，不随 toast 消失。 */
  const [failure, setFailure] = useState<string | null>(null);

  // 每次打开弹窗重置为全选（目标集合可能因装/卸插件而变化，不保留上次勾选）。
  useEffect(() => {
    if (open) {
      setSelected(null);
      setFailure(null);
    }
  }, [open, account?.id]);

  const checked = selected ?? targets.map((t) => t.configDir);
  const allChecked = targets.length > 0 && checked.length === targets.length;
  /** 有未映射的运行实例时禁止切换（与后端「拒绝写入」一致，避免白跑一次）。 */
  const blocked = unmapped.length > 0;

  function toggle(dir: string, on: boolean) {
    setSelected((prev) => {
      const base = prev ?? targets.map((t) => t.configDir);
      return on ? [...new Set([...base, dir])] : base.filter((d) => d !== dir);
    });
  }

  async function onConfirm() {
    if (!account || checked.length === 0 || busy) return;
    setBusy(true);
    setFailure(null);
    const toastId = toast.loading("正在切换 JetBrains IDE…", {
      description: "将注入凭证，运行中的 IDE / Android Studio 会先退出再自动重开",
    });
    try {
      const result = await api.switchJetbrainsAccount(
        account.id,
        true,
        checked.length === targets.length ? undefined : checked,
      );
      await onDone?.();
      toast.success("JetBrains IDE 已切换", {
        id: toastId,
        description: result.message || result.account,
      });
      onOpenChange(false);
    } catch (error) {
      const text = api.asError(error);
      // 失败原文可能有多行（已识别几个 IDE / 已回滚哪些目标）：留在弹窗里，别只丢给 toast。
      setFailure(text);
      toast.error("JetBrains IDE 切换失败", {
        id: toastId,
        description: "详细原因已显示在弹窗中，可复制反馈。",
      });
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <JetbrainsMark size={20} />
            切换 JetBrains 插件账号
          </DialogTitle>
          <DialogDescription>
            {account ? (
              <>把 {displayName(account)} 设为所选 IDE 的 CodeBuddy 插件登录账号。</>
            ) : (
              "选择要写入的 IDE。"
            )}
          </DialogDescription>
        </DialogHeader>

        {failure && (
          <Alert variant="destructive" className="min-w-0">
            <AlertTriangle />
            <AlertTitle>切换失败</AlertTitle>
            <AlertDescription className="w-full min-w-0">
              <p className="max-h-40 w-full overflow-auto whitespace-pre-line break-words rounded bg-background/70 p-2 text-xs leading-5">
                {failure}
              </p>
              <p className="text-xs leading-5">
                以上信息不含任何 token，可直接复制反馈（日志：<code>error.log</code>）。
              </p>
            </AlertDescription>
          </Alert>
        )}

        {blocked && (
          <div className="space-y-1.5 rounded-lg border border-warning/40 bg-warning-soft/60 px-3 py-2 text-sm">
            <p className="flex items-center gap-1.5 font-medium text-warning">
              <AlertTriangle className="size-4" aria-hidden />
              有 {unmapped.length} 个运行中的 IDE 无法识别，已禁止切换
            </p>
            <ul className="space-y-0.5 text-xs leading-5 text-muted-foreground">
              {unmapped.map((proc) => (
                <li key={`${proc.pid}-${proc.path ?? ""}`}>
                  {proc.product ?? "未知 IDE"}
                  {proc.version ? ` ${proc.version}` : ""}（PID {proc.pid}）
                  {proc.path ? ` ${proc.path}` : ""}
                </li>
              ))}
            </ul>
            <p className="text-xs leading-5 text-muted-foreground">
              写入会被这些 IDE 的内存态覆盖，因此先拒绝写入。请
              <strong className="font-medium text-foreground">完全退出</strong>
              以上进程后重新打开本弹窗。
            </p>
          </div>
        )}

        {targets.length === 0 ? (
          <div className="space-y-2">
            <p className="rounded-lg bg-muted/60 px-3 py-2 text-sm text-muted-foreground">
              未检测到已安装 CodeBuddy 插件的 JetBrains IDE。请先在 JetBrains 系 IDE（IDEA /
              PyCharm / GoLand 等）或 Android Studio 中安装「Tencent Cloud CodeBuddy」插件后重试。
            </p>
            <JetbrainsDiagnosticsPanel status={jetbrainsStatus} defaultOpen />
          </div>
        ) : (
          <div className="space-y-1">
            <label className="flex cursor-pointer items-center gap-2.5 rounded-lg px-2 py-1.5 text-sm font-medium hover:bg-muted/60">
              <Checkbox
                checked={allChecked}
                onCheckedChange={(on) => setSelected(on ? null : [])}
                aria-label="全选"
              />
              全部 IDE / Android Studio
            </label>
            {targets.map((t) => (
              <label
                key={t.configDir}
                className={cn(
                  "flex cursor-pointer items-center gap-2.5 rounded-lg px-2 py-1.5 text-sm hover:bg-muted/60",
                  checked.length === 0 && "opacity-60",
                )}
              >
                <Checkbox
                  checked={checked.includes(t.configDir)}
                  onCheckedChange={(on) => toggle(t.configDir, Boolean(on))}
                  aria-label={`写入 ${t.configDir}`}
                />
                <span className="min-w-0 flex-1 truncate" title={t.configDir}>
                  {productLabel(t.configDir)}
                </span>
                {t.running && (
                  <span className="flex-none rounded-full bg-primary/10 px-2 py-0.5 text-xs text-primary">
                    运行中
                  </span>
                )}
              </label>
            ))}
            <p className="px-2 pt-1 text-xs leading-5 text-muted-foreground">
              运行中的 IDE 会先退出（弹「确认退出」时自动确认）再写入并重新打开；未运行的 IDE
              写入后下次打开生效。Android Studio 与 JetBrains 系 IDE 共用同一份账号。
            </p>
            <div className="pt-1">
              <JetbrainsDiagnosticsPanel status={jetbrainsStatus} />
            </div>
          </div>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
            取消
          </Button>
          <Button
            onClick={onConfirm}
            disabled={busy || blocked || targets.length === 0 || checked.length === 0}
            title={blocked ? "请先完全退出无法识别的 JetBrains IDE" : undefined}
          >
            {busy ? <Loader2 className="size-4 animate-spin" /> : null}
            {busy ? "切换中…" : `切换（${checked.length}/${targets.length}）`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
