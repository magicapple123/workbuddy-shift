import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import { changeCompanionEnabled, reloadCompanionEnabled, useCompanionEnabled } from "@/hooks/use-companion-enabled";
import { SettingsFieldRow, SettingsGroup } from "./rows";

/** 桌面版内置会话悬浮窗：状态以宿主后端的持久化结果为准。 */
export function CompanionCard() {
  const { enabled, busy, error } = useCompanionEnabled();

  async function onToggle(next: boolean) {
    try {
      const confirmed = await changeCompanionEnabled(next, "settings");
      toast.success(confirmed ? "已启用会话悬浮窗" : "已关闭会话悬浮窗");
    } catch (cause) {
      toast.error("悬浮窗设置失败", { description: api.asError(cause) });
    }
  }

  async function openSettings() {
    try {
      await api.openCompanionSettings();
    } catch (error) {
      toast.error("打开悬浮窗设置失败", { description: api.asError(error) });
    }
  }

  return (
    <SettingsGroup id="settings-companion" title="会话悬浮窗">
      <CardContent className="space-y-0 p-0">
        <SettingsFieldRow
          className="border-b-0"
          label="启用会话悬浮窗"
          description="启用后显示悬浮栏；开机静默启动时也会显示，可从托盘临时隐藏"
          htmlFor="companion-enabled"
        >
          <div className="flex items-center gap-2">
            {error && enabled === null ? (
              <Button size="sm" variant="outline" onClick={() => void reloadCompanionEnabled()}>重试</Button>
            ) : null}
            {enabled ? (
              <Button size="sm" variant="outline" onClick={() => void openSettings()}>悬浮窗设置</Button>
            ) : null}
            <Switch
              id="companion-enabled"
              checked={enabled ?? false}
              disabled={busy || enabled === null}
              onCheckedChange={(value) => void onToggle(value)}
              aria-label="启用会话悬浮窗"
            />
          </div>
        </SettingsFieldRow>
      </CardContent>
    </SettingsGroup>
  );
}
