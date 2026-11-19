import { Fragment, useCallback, useEffect, useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { DemoAction } from "@/components/demo-action";
import * as api from "@/lib/api";
import type {
  SettingsSyncAccount,
  SettingsSyncPlan,
  SettingsSyncPlanTarget,
  SettingsSyncStatus,
} from "@/lib/types";

import { SettingsGroup } from "./rows";

/** 主题取值的中文化；与「外观」卡的下拉选项同一套词，未知值原样显示。 */
function themeLabel(theme: string | null): string {
  switch (theme) {
    case null:
    case "":
      return "—";
    case "light":
      return "浅色";
    case "dark":
      return "深色";
    case "system":
      return "跟随系统";
    default:
      return theme;
  }
}

/**
 * `scopes` 是 `storage/user-<uid>-personal/scoped/<id>` 下的目录名（内部 id）。
 * 业务含义未知，因此**不进正文**，只作为「N 组设置」的 tooltip 技术细节。
 */
function scopesTitle(scopes: string[]): string | undefined {
  return scopes.length > 0 ? `设置组：${scopes.join("、")}` : undefined;
}

/** 预览行：以 `status.accounts` 为准，按 uid 关联计划项。 */
interface PreviewRow {
  account: SettingsSyncAccount;
  target: SettingsSyncPlanTarget | null;
}

/**
 * 设置跨账号同步：切号后界面 / 通用设置保持一致。
 *
 * 只同步 `preferences.json`（主题、外观资源、权限模式、提示音、语言）；扩展刷新游标
 * 等设备本地状态由服务端 denylist 跳过。同步前自动备份目标文件，备份失败零写入。
 *
 * 展示层（第三轮第 13 条）：上表是「账号属性表」，下面是「执行计划」，两者职责不同，
 * 因此分开排版 —— 表格固定四列 + 限高 + 粘性表头，预览区与表格同列对齐，并用
 * 「本次预览 / 上次结果」各自成块，避免 12 行表格与 10 行预览连成一片。
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

  /**
   * 预览行：按 `status.accounts` 的顺序遍历，源账号与「无目录且不在计划里」的账号不出现；
   * 这样表格与预览的顺序永远一致，脚注也就解释得清「为什么少了两行」。
   */
  const previewRows: PreviewRow[] = plan && status
    ? status.accounts
      .filter((account) => account.uid !== plan.source.uid)
      .map((account) => ({
        account,
        target: plan.targets.find((target) => target.uid === account.uid) ?? null,
      }))
      .filter((row) => row.account.hasDirectory || row.target !== null)
    : [];

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
            <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs font-medium tabular-nums text-muted-foreground">
                {status.accounts.length} 个账号
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
                    同步到所有账号
                  </Button>
                </DemoAction>
              </div>
            </div>

            {/* 账号属性表：固定列宽 + 限高 + 粘性表头；状态列只放文字，内部 id 进 tooltip。 */}
            <div className="max-h-[320px] overflow-y-auto rounded-lg border border-border/60">
              <table className="w-full table-fixed text-xs">
                <colgroup>
                  <col className="w-[40%]" />
                  <col className="w-[14%]" />
                  <col className="w-[20%]" />
                  <col className="w-[26%]" />
                </colgroup>
                <thead className="sticky top-0 z-10 bg-muted text-[11px] text-muted-foreground">
                  <tr>
                    <th className="px-2 py-1.5 text-left font-medium">账号</th>
                    <th className="px-2 py-1.5 text-left font-medium">主题</th>
                    <th className="px-2 py-1.5 text-left font-medium">外观资源</th>
                    <th className="px-2 py-1.5 text-left font-medium">本机状态</th>
                  </tr>
                </thead>
                <tbody>
                  {status.accounts.map((account) => (
                    <tr key={account.uid} className="border-t border-border/40">
                      <td className="truncate px-2 py-1.5" title={account.label}>
                        {account.label}
                      </td>
                      <td className="truncate px-2 py-1.5">{themeLabel(account.theme)}</td>
                      <td
                        className="truncate px-2 py-1.5 font-mono text-[10px]"
                        title={account.themeResource ?? undefined}
                      >
                        {account.themeResource ?? "—"}
                      </td>
                      <td className="px-2 py-1.5">
                        {account.hasDirectory ? (
                          <span
                            className="flex min-w-0 items-baseline gap-1"
                            title={scopesTitle(account.scopes)}
                          >
                            <span className="shrink-0">可同步</span>
                            <span className="min-w-0 truncate text-[10px] text-muted-foreground">
                              · {account.scopes.length} 组设置
                            </span>
                          </span>
                        ) : (
                          <span className="text-muted-foreground">未登录，跳过</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {plan ? (
              <div className="mt-3 rounded-lg border border-border/60 bg-muted/20 p-2.5 text-[11px]">
                <div className="mb-1.5 flex flex-wrap items-baseline justify-between gap-2">
                  <span className="font-medium text-foreground">
                    本次预览
                    <span className="ml-1 font-normal text-muted-foreground" title={plan.source.uid}>
                      · 源账号：{plan.source.label}
                    </span>
                  </span>
                  <span className="shrink-0 tabular-nums text-muted-foreground">
                    待同步 {pending} 项
                  </span>
                </div>
                <div className="grid grid-cols-[40%_minmax(0,1.6fr)_auto] items-baseline gap-x-2 gap-y-1">
                  {previewRows.map(({ account, target }) => {
                    const overwrite =
                      target?.changes.filter((change) => change.action === "overwrite").length ?? 0;
                    const added =
                      target?.changes.filter((change) => change.action === "add").length ?? 0;
                    const themeRewritten =
                      target?.changes.some((change) => change.key === "appearance.themeSelection") ??
                      false;
                    return (
                      <Fragment key={account.uid}>
                        <span className="truncate" title={account.label}>
                          {account.label}
                        </span>
                        <span className="min-w-0">
                          {!target ? (
                            <span className="text-muted-foreground">不在本次计划内</span>
                          ) : target.skipped ? (
                            <span className="text-muted-foreground">跳过：{target.reason}</span>
                          ) : target.changes.length === 0 ? (
                            <span className="text-muted-foreground">已一致</span>
                          ) : (
                            <span className="tabular-nums">
                              覆盖 {overwrite} 项 · 新增 {added} 项
                            </span>
                          )}
                        </span>
                        <span className="justify-self-end">
                          {themeRewritten && (
                            <Badge variant="secondary" className="text-[10px] font-normal">
                              主题归属改写
                            </Badge>
                          )}
                        </span>
                      </Fragment>
                    );
                  })}
                </div>
                <p className="mt-1.5 text-[10px] leading-4 text-muted-foreground/80">
                  源账号与未登录账号不参与同步，因此这里的行数比上表少。
                </p>
              </div>
            ) : (
              <p className="mt-3 text-[11px] text-muted-foreground">
                尚无预览，点「预览差异」查看将变更的内容。
              </p>
            )}

            <div className="mt-3 border-t border-border/60 pt-2.5 text-[11px] text-muted-foreground">
              <div className="mb-1 text-[11px] font-medium text-foreground">上次结果</div>
              {status.lastResult ? (
                <>
                  <div>
                    最近一次同步：
                    {new Date(status.lastResult.at).toLocaleString("zh-CN")} · 备份根{" "}
                    <span className="font-mono">{status.lastResult.backupRoot}</span>
                  </div>
                  {status.lastResult.targets.slice(0, 3).map((target) => (
                    <div key={target.uid} className="break-all">
                      {target.label}：
                      {target.skipped
                        ? `跳过（${target.reason}）`
                        : `${target.written?.length ?? 0} 个文件`}
                      {target.backup ? ` · ${target.backup}` : ""}
                    </div>
                  ))}
                </>
              ) : (
                <div>还没有执行过同步。</div>
              )}
            </div>
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
