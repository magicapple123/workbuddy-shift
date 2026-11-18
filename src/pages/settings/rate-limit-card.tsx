import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import type {
  RateLimitConfig,
  RateLimitHookStatus,
} from "@/lib/types";

import { SettingsFieldRow, SettingsGroup } from "./rows";

/** 限额监听：总开关 + hook 接入状态（CLI / WorkBuddy 实时上报，IDE 仍走日志扫描）。 */
export function RateLimitCard() {
  const [config, setConfig] = useState<RateLimitConfig | null>(null);
  const [status, setStatus] = useState<RateLimitHookStatus | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([api.getRateLimitConfig(), api.getRateLimitHookStatus()])
      .then(([cfg, hook]) => {
        if (cancelled) return;
        setConfig(cfg);
        setStatus(hook);
      })
      .catch((e) => {
        if (!cancelled) toast.error("限额监听配置加载失败", { description: api.asError(e) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function onToggle(enabled: boolean) {
    if (!config || busy) return;
    const previous = config;
    setConfig({ ...config, enabled });
    setBusy(true);
    try {
      // 整个配置一起提交：只带 enabled 会把「卸载过」标记冲掉，重启后 hook 又被自动装回。
      setConfig(await api.saveRateLimitConfig({ ...config, enabled }));
      toast.success(enabled ? "限额监听已开启" : "限额监听已关闭");
    } catch (e) {
      setConfig(previous);
      toast.error("限额监听设置保存失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  /**
   * 「扫描 CodeBuddy IDE 日志」独立开关：只关两个 IDE 的日志来源（IDE 的 429 不触发任何
   * 事件，日志是它唯一的数据源），CLI / WorkBuddy 的 hook 通路不受影响。
   */
  async function onToggleIdeLogs(scanIdeLogs: boolean) {
    if (!config || busy) return;
    const previous = config;
    setConfig({ ...config, scanIdeLogs });
    setBusy(true);
    try {
      // 与总开关一样整份提交：只带 scanIdeLogs 会把 enabled / hookOptOut 冲成默认值。
      setConfig(await api.saveRateLimitConfig({ ...config, scanIdeLogs }));
      toast.success(
        scanIdeLogs
          ? "已开启 IDE 日志扫描"
          : "已关闭 IDE 日志扫描：两个 CodeBuddy IDE 的限额不再显示",
      );
    } catch (e) {
      setConfig(previous);
      toast.error("限额监听设置保存失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  /**
   * 回读限额配置：装 / 卸 hook 无论成败都会写「接入 / 卸载」意图（部分目标失败也算），
   * 本地标记不能只靠乐观更新，否则随后拨总开关会把过期值写回磁盘。
   */
  function refreshHookConfig() {
    return api
      .getRateLimitConfig()
      .then(setConfig)
      .catch(() => {
        /* 读不到就保持本地值，下次进设置页会重新拉 */
      });
  }

  async function onInstall() {
    if (busy) return;
    setBusy(true);
    try {
      setStatus(await api.installRateLimitHook());
      toast.success("已接入限额监听", {
        description: "CodeBuddy CLI / WorkBuddy 的 429 会实时上报（原配置已备份，可随时卸载还原）",
      });
    } catch (e) {
      toast.error("接入 hook 失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
      void refreshHookConfig();
    }
  }

  async function onUninstall() {
    if (busy) return;
    setBusy(true);
    try {
      setStatus(await api.uninstallRateLimitHook());
      toast.success("已卸载 hook", {
        description: "客户端配置恢复原状，之后不会再自动接入（限额改由日志扫描发现，可随时点「接入 hook」恢复）",
      });
    } catch (e) {
      toast.error("卸载 hook 失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
      void refreshHookConfig();
    }
  }

  const existingTargets = status?.targets.filter((target) => target.exists) ?? [];
  const installedCount = existingTargets.filter((target) => target.installed).length;
  // IDE 的限额只有日志一条来源：扫描开关关闭时文案不能再说「仍按日志扫描」。
  const ideNote =
    config?.scanIdeLogs === false
      ? "CodeBuddy IDE 的日志扫描已关闭"
      : "CodeBuddy IDE 无事件，仍按日志扫描";
  const hookDescription = status
    ? existingTargets.length === 0
      ? `未检测到 CodeBuddy CLI / WorkBuddy 客户端：没有可接入的配置（${ideNote}）`
      : status.installed
        ? `${installedCount} / ${existingTargets.length} 个已安装客户端已接入：429 当轮实时上报（秒级）；${ideNote}`
        : config?.hookOptOut
          ? "已卸载：不会再自动接入，限额改由日志扫描发现；点「接入 hook」可恢复实时上报"
          : "未接入：限额仅靠定期扫描日志发现（最多滞后数分钟）"
    : "加载中…";

  return (
    <SettingsGroup id="settings-rate-limit" title="限额监听">
      <CardContent className="space-y-0 p-0">
        <SettingsFieldRow
          label="启用限额监听"
          description="关闭后不扫描日志、账号卡片不显示限额标记；重新开启后恢复"
          htmlFor="rl-enabled"
          operational
        >
          <Switch
            id="rl-enabled"
            checked={config?.enabled ?? true}
            disabled={busy || !config}
            onCheckedChange={(v) => void onToggle(v)}
            aria-label="启用限额监听"
          />
        </SettingsFieldRow>

        <SettingsFieldRow
          label="扫描 CodeBuddy IDE 日志"
          description="IDE 的限额只有日志一条来源，关掉后不再显示；CodeBuddy CLI / WorkBuddy 的实时上报不受影响"
          htmlFor="rl-ide-scan"
          operational
        >
          <Switch
            id="rl-ide-scan"
            checked={config?.scanIdeLogs ?? true}
            disabled={busy || !config}
            onCheckedChange={(v) => void onToggleIdeLogs(v)}
            aria-label="扫描 CodeBuddy IDE 日志"
          />
        </SettingsFieldRow>

        <SettingsFieldRow
          className="border-b-0"
          label="接入客户端 hook"
          description={hookDescription}
          operational
        >
          {status?.installed ? (
            <Button
              id="rl-hook"
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void onUninstall()}
            >
              {busy ? <Loader2 className="animate-spin" /> : null}卸载 hook
            </Button>
          ) : (
            <Button
              id="rl-hook"
              size="sm"
              disabled={busy || !status || existingTargets.length === 0}
              onClick={() => void onInstall()}
            >
              {busy ? <Loader2 className="animate-spin" /> : null}接入 hook
            </Button>
          )}
        </SettingsFieldRow>
      </CardContent>
    </SettingsGroup>
  );
}
