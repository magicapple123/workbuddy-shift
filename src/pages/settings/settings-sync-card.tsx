import { useCallback, useEffect, useState } from "react";
import { Loader2, RefreshCw, ShieldCheck } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { DemoAction } from "@/components/demo-action";
import * as api from "@/lib/api";
import type { SettingsSyncPlan, SettingsSyncStatus } from "@/lib/types";

import { SettingsGroup } from "./rows";

/**
 * 设置跨账号同步：切号后界面 / 通用设置保持一致。
 *
 * 只同步 `preferences.json`（主题、外观资源、权限模式、提示音、语言）；扩展刷新游标
 * 等设备本地状态由服务端 denylist 跳过。同步前自动备份目标文件，备份失败零写入。
 */
export function SettingsSyncCard() {
  const [status, setStatus] = useState<SettingsSyncStatus | null>(null);
  const [plan, setPlan] = useState<SettingsSyncPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setStatus(await api.getSettingsSyncStatus());
      setError(null);
    } catch (cause) {
      setError(api.asError(cause));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function toggle(patch: { enabled?: boolean; autoOnSwitch?: boolean }) {
    setBusy(true);
    try {
      await api.saveSettingsSyncConfig(patch);
      await load();
    } catch (cause) {
      toast.error("保存失败", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }

  async function preview() {
    setBusy(true);
    try {
      const result = await api.previewSettingsSync();
      setPlan(result);
      const changed = result.targets.filter((target) => !target.skipped && target.changes.length > 0);
      toast.info(
        changed.length === 0
          ? "目标账号设置已与当前账号一致"
          : `将更新 ${changed.length} 个账号`,
      );
    } catch (cause) {
      toast.error("预览失败", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }

  async function apply() {
    setBusy(true);
    try {
      const result = await api.syncSettings(undefined, false);
      const written = result.targets.filter((target) => (target.written?.length ?? 0) > 0);
      if (written.length === 0) {
        toast.info("没有需要写入的账号");
      } else {
        toast.success(`已同步 ${written.length} 个账号`, {
          description: written[0]?.backup ? `备份：${written[0].backup}` : undefined,
        });
      }
      const skipped = result.targets.filter((target) => target.skipped);
      for (const target of skipped.slice(0, 3)) {
        toast.info(`${target.label} 已跳过`, { description: target.reason });
      }
      setPlan(null);
      await load();
    } catch (cause) {
      toast.error("同步失败（未写入）", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }

  const config = status?.config;
  const pending = plan
    ? plan.targets.reduce((sum, target) => sum + (target.skipped ? 0 : target.changes.length), 0)
    : 0;

  return (
    <SettingsGroup id="settings-sync" title="设置跨账号同步">
      <CardContent className="space-y-0 p-0">
        <div className="border-b border-border/50 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
          切换账号时把当前账号的界面 / 通用设置（主题、外观资源、权限模式、提示音、语言）同步到目标账号。
          只同步 <code className="font-mono">preferences.json</code>，不同步专家、技能与全局客户端配置；
          覆盖前自动备份，备份失败则不写入。
        </div>

        <div className="mx-4 flex items-center justify-between border-b border-border/50 py-2.5 sm:mx-5">
          <div className="min-w-0">
            <div className="text-sm">启用设置同步</div>
            <div className="text-xs text-muted-foreground">关闭后切号不再自动同步，也不允许手动执行。</div>
          </div>
          <Switch
            checked={config?.enabled ?? false}
            disabled={busy || !status}
            onCheckedChange={(checked) => void toggle({ enabled: checked })}
            aria-label="启用设置同步"
          />
        </div>

        <div className="mx-4 flex items-center justify-between border-b border-border/50 py-2.5 sm:mx-5">
          <div className="min-w-0">
            <div className="text-sm">切号时自动同步</div>
            <div className="text-xs text-muted-foreground">仅在勾选「重启客户端」的切换中生效。</div>
          </div>
          <Switch
            checked={config?.autoOnSwitch ?? false}
            disabled={busy || !status || !config?.enabled}
            onCheckedChange={(checked) => void toggle({ autoOnSwitch: checked })}
            aria-label="切号时自动同步"
          />
        </div>

        {error ? (
          <div className="px-4 py-3 text-xs text-destructive sm:px-5">{error}</div>
        ) : null}

        {status ? (
          <div className="px-4 py-3 sm:px-5">
            <div className="mb-2 flex items-center justify-between">
              <span className="text-xs font-medium text-muted-foreground">
                账号（{status.accounts.length}）
              </span>
              <div className="flex gap-2">
                <DemoAction>
                  <Button size="sm" variant="ghost" disabled={busy} onClick={() => void load()}>
                    <RefreshCw className="size-3.5" />
                    刷新
                  </Button>
                </DemoAction>
                <DemoAction>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy || !config?.enabled}
                    onClick={() => void preview()}
                  >
                    预览差异
                  </Button>
                </DemoAction>
                <DemoAction>
                  <Button
                    size="sm"
                    disabled={busy || !config?.enabled || !plan || pending === 0}
                    onClick={() => void apply()}
                  >
                    {busy ? <Loader2 className="size-3.5 animate-spin" /> : null}
                    同步到所有账号{pending > 0 ? `（${pending} 项）` : ""}
                  </Button>
                </DemoAction>
              </div>
            </div>

            <div className="overflow-hidden rounded-lg border border-border/60">
              <table className="w-full text-xs">
                <thead className="bg-muted/40 text-[11px] text-muted-foreground">
                  <tr>
                    <th className="px-2 py-1.5 text-left font-medium">账号</th>
                    <th className="px-2 py-1.5 text-left font-medium">主题</th>
                    <th className="px-2 py-1.5 text-left font-medium">外观资源</th>
                    <th className="px-2 py-1.5 text-left font-medium">目录</th>
                  </tr>
                </thead>
                <tbody>
                  {status.accounts.map((account) => (
                    <tr key={account.uid} className="border-t border-border/40">
                      <td className="max-w-[220px] truncate px-2 py-1.5">{account.label}</td>
                      <td className="px-2 py-1.5">{account.theme ?? "—"}</td>
                      <td className="px-2 py-1.5 font-mono text-[10px]">
                        {account.themeResource ?? "—"}
                      </td>
                      <td className="px-2 py-1.5">
                        {account.hasDirectory ? (
                          <Badge variant="secondary" className="text-[10px]">
                            <ShieldCheck className="mr-1 size-3" />
                            {account.scopes.length} 个 scope
                          </Badge>
                        ) : (
                          <span className="text-muted-foreground">未在本机登录，将跳过</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {plan ? (
              <div className="mt-3 space-y-1 rounded-lg border border-border/60 bg-muted/20 p-2 text-[11px]">
                <div className="text-muted-foreground">
                  源账号：{plan.source.label}（{plan.source.uid}）
                </div>
                {plan.targets.map((target) => (
                  <div key={target.uid} className="flex flex-wrap gap-x-2">
                    <span className="w-40 truncate">{target.label}</span>
                    {target.skipped ? (
                      <span className="text-muted-foreground">跳过：{target.reason}</span>
                    ) : target.changes.length === 0 ? (
                      <span className="text-muted-foreground">已一致</span>
                    ) : (
                      <span>
                        {target.changes.filter((change) => change.action === "overwrite").length} 项覆盖 ·{" "}
                        {target.changes.filter((change) => change.action === "add").length} 项新增
                        {target.changes.some((change) => change.key === "appearance.themeSelection")
                          ? " · 主题归属改写为目标 uid"
                          : ""}
                      </span>
                    )}
                  </div>
                ))}
              </div>
            ) : null}

            {status.lastResult ? (
              <div className="mt-3 space-y-0.5 text-[11px] text-muted-foreground">
                <div>
                  最近一次同步：
                  {new Date(status.lastResult.at).toLocaleString("zh-CN")} · 备份根{" "}
                  <span className="font-mono">{status.lastResult.backupRoot}</span>
                </div>
                {status.lastResult.targets.slice(0, 3).map((target) => (
                  <div key={target.uid} className="break-all">
                    {target.label}：{target.skipped ? `跳过（${target.reason}）` : `${target.written?.length ?? 0} 个文件`}
                    {target.backup ? ` · ${target.backup}` : ""}
                  </div>
                ))}
              </div>
            ) : (
              <div className="mt-3 text-[11px] text-muted-foreground">还没有执行过同步。</div>
            )}
          </div>
        ) : (
          <div className="flex items-center gap-2 px-4 py-4 text-xs text-muted-foreground sm:px-5">
            <Loader2 className="size-3.5 animate-spin" /> 正在读取设置同步状态…
          </div>
        )}
      </CardContent>
    </SettingsGroup>
  );
}
