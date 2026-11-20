import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import { SettingsFieldRow, SettingsGroup } from "./rows";

/**
 * 切号时同步 WorkBuddy「自动化」定时任务（默认关闭）。
 *
 * 任务按 `owner_user_id` 绑账号，切号后原账号的任务不再执行且客户端不提示。开关关闭时
 * 只在切号弹窗里提示「有 N 个任务会停止」；开启后切号会把**简单任务**复制到目标账号，
 * 并把源任务改为暂停（不删除）。含技能 / 连接器 / 空间引用的任务只提示、不复制——
 * 这些引用在目标账号下大概率不存在。
 *
 * 只在勾选「重启客户端」时生效：客户端没关就写库会和它自己的调度器打架。
 */
export function AutomationSyncCard() {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    api
      .automationSyncEnabled()
      .then((res) => {
        if (!cancelled) setEnabled(res.enabled);
      })
      .catch((e) => {
        if (!cancelled) setError(api.asError(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function toggle(next: boolean) {
    setBusy(true);
    setError("");
    try {
      const res = await api.automationSyncSetEnabled(next);
      setEnabled(res.enabled);
      toast.success(res.enabled ? "切号时将同步定时任务" : "切号时不再同步定时任务");
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsGroup id="settings-automation-sync" title="定时任务">
      <SettingsFieldRow
        label="切号时同步 WorkBuddy 定时任务"
        description={
          <span className="space-y-1">
            <span className="block">
              开启后，切号会把当前账号可搬运的定时任务复制到目标账号，并把原任务改为暂停（不删除）；
              含技能、连接器或项目空间引用的任务只提示、不复制。
            </span>
            <span className="block">
              只在勾选「重启客户端」时执行；关闭开关时仍会在切号弹窗提示有几个任务会停止。
            </span>
            {error ? <span className="block text-danger">{error}</span> : null}
          </span>
        }
        htmlFor="automation-sync-switch"
      >
        <Switch
          id="automation-sync-switch"
          checked={enabled === true}
          disabled={busy || enabled === null}
          onCheckedChange={(next) => void toggle(next)}
        />
      </SettingsFieldRow>
    </SettingsGroup>
  );
}
