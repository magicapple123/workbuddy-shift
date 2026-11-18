import { useEffect, useState } from "react";
import { toast } from "sonner";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import { SettingsFieldRow, SettingsGroup } from "./rows";

/** 开机自启（仅桌面端渲染）：开关直接反映系统自启注册状态，切换立即生效。 */
export function StartupCard() {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void api
      .getLaunchAtLoginEnabled()
      .then((value) => {
        if (!cancelled) setEnabled(value);
      })
      .catch((e) => {
        if (!cancelled) toast.error("开机自启状态读取失败", { description: api.asError(e) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function onToggle(value: boolean) {
    if (busy || enabled === null) return;
    const previous = enabled;
    setBusy(true);
    try {
      // 后端回读 OS 权威状态；即使与请求一致，也以回读值显示。
      const authoritative = await api.setLaunchAtLoginEnabled(value);
      setEnabled(authoritative);
      toast.success(authoritative ? "已开启开机自启" : "已关闭开机自启");
    } catch (e) {
      // 失败时恢复到最后一次确认的状态，并显示可读错误。
      setEnabled(previous);
      toast.error("开机自启设置失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsGroup
      id="settings-startup"
      title="启动设置"
    >
      <CardContent className="space-y-0 p-0">
        <SettingsFieldRow
          className="border-b-0"
          label="开机时静默启动到托盘"
          description="开关直接反映系统登录项状态；之后可从托盘「打开主界面」恢复"
          htmlFor="startup-silent"
          operational
        >
          <Switch
            id="startup-silent"
            checked={enabled ?? false}
            disabled={busy || enabled === null}
            onCheckedChange={(v) => void onToggle(v)}
            aria-label="开机时静默启动到托盘"
          />
        </SettingsFieldRow>
      </CardContent>
    </SettingsGroup>
  );
}
