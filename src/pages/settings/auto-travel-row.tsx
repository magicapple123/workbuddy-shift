import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import type { TravelConfig } from "@/lib/types";
import { SettingsFieldRow } from "./rows";

/**
 * 自动旅行：单行开关，渲染在「自动签到」卡片内部（不独立成卡）。
 *
 * 成长中心仅国内版开放，调用方按档位决定是否渲染；开关切换即落盘（无「保存配置」按钮），
 * 失败回滚并提示。
 */
export function AutoTravelRow() {
  const [cfg, setCfg] = useState<TravelConfig | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void api
      .getAutoTravelConfig()
      .then((config) => {
        if (!cancelled) setCfg(config);
      })
      .catch((e) => {
        if (!cancelled) toast.error("自动旅行配置加载失败", { description: api.asError(e) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function onToggle(enabled: boolean) {
    if (!cfg || busy) return;
    const previous = cfg;
    setCfg({ ...cfg, enabled });
    setBusy(true);
    try {
      setCfg(await api.saveAutoTravelConfig({ ...cfg, enabled }));
      if (enabled) {
        toast.success("自动旅行已开启", { description: "正在按官方状态派发或领取" });
      } else {
        toast.success("自动旅行已关闭");
      }
    } catch (e) {
      setCfg(previous);
      toast.error("自动旅行设置保存失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsFieldRow
      label="启用自动旅行"
      description="开启后按官方状态自动派发或领取旅行奖励；切换后立即生效"
      htmlFor="at-enabled"
      operational
    >
      <Switch
        id="at-enabled"
        checked={cfg?.enabled ?? false}
        disabled={busy || !cfg}
        onCheckedChange={(v) => void onToggle(v)}
        aria-label="启用自动旅行"
      />
    </SettingsFieldRow>
  );
}
