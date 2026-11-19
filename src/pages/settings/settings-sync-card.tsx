import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { ChevronDown, ChevronRight, Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { DemoAction } from "@/components/demo-action";
import * as api from "@/lib/api";
import type {
  SettingsSyncAccount,
  SettingsSyncBackupEntry,
  SettingsSyncLastResult,
  SettingsSyncPlan,
  SettingsSyncPlanTarget,
  SettingsSyncStatus,
} from "@/lib/types";

import { SettingsGroup } from "./rows";

/** 主题键里会出现「主题归属改写」，预览/确认弹窗都要用到这个字面量。 */
const THEME_KEY = "appearance.themeSelection";

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
 * 主题单元格：`浅色（light）`；付费主题把资源名放进括号（`浅色（theme-tkbdx8）`）。
 *
 * 第三轮整改：原来「主题」与「外观资源」两列语义重复（light / light），
 * 合并成一列后按「档位（资源名）」显示，一列就看得出「什么色 + 哪个主题包」。
 */
function themeCell(account: SettingsSyncAccount): string {
  const base = themeLabel(account.theme);
  const detail = account.themeResource ?? account.theme;
  return detail ? `${base}（${detail}）` : base;
}

/**
 * `scopes` 是 `storage/user-<uid>-personal/scoped/<id>` 下的目录名（内部 id）。
 * 业务含义未知，因此**不进正文**，只作为「N 个工作区配置」的 tooltip 技术细节。
 */
function scopesTitle(scopes: string[]): string {
  return scopes.length > 0
    ? `本机存在的工作区配置目录（scoped/ 下的目录名）：${scopes.join("、")}`
    : "本机没有任何工作区级配置目录，只会同步全局 preferences.json";
}

/** 从 `appearance.themeSelection` 的任意版本里取出资源名；拿不到返回 null。 */
function resourceKeyOf(value: unknown): string | null {
  if (!value || typeof value !== "object") return null;
  const key = (value as { resourceKey?: unknown }).resourceKey;
  return typeof key === "string" && key.length > 0 ? key : null;
}

/** 预览行：以 `status.accounts` 为准，按 uid 关联计划项。 */
interface PreviewRow {
  account: SettingsSyncAccount;
  target: SettingsSyncPlanTarget | null;
}

/** 「上次结果」的一句话：来源 · 结果 · 账号数（时间由调用处补）。 */
function resultSummary(result: SettingsSyncLastResult): string {
  const trigger = result.trigger === "auto" ? "切换时自动" : "手动";
  const synced =
    result.counts?.synced ?? result.targets.filter((target) => target.status === "synced").length;
  const reason = result.reason ? `（${result.reason}）` : "";
  switch (result.status) {
    case "skipped":
      return `${trigger} · 跳过${reason}`;
    case "rejected":
      return `${trigger} · 已拒绝${reason}`;
    case "failed":
      return `${trigger} · 失败${reason}`;
    case "unchanged":
      return `${trigger} · 目标已一致，未写入`;
    case "partial":
      return `${trigger} · ${synced} 个账号（部分失败）`;
    default:
      return `${trigger} · ${synced} 个账号`;
  }
}

/** 备份时间戳 `YYYYMMDD-HHMMSS` → 本地时间；解析失败原样返回。 */
function formatStamp(stamp: string, createdAt?: number | null): string {
  if (typeof createdAt === "number" && createdAt > 0) {
    return new Date(createdAt).toLocaleString("zh-CN", { hour12: false });
  }
  const match = /^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})$/.exec(stamp);
  if (!match) return stamp;
  const [, year, month, day, hour, minute, second] = match;
  return `${year}-${month}-${day} ${hour}:${minute}:${second}`;
}

/**
 * 设置跨账号同步：切号后界面 / 通用设置保持一致。
 *
 * 只同步 `preferences.json`（主题、外观资源、权限模式、提示音、语言）；扩展刷新游标
 * 等设备本地状态由服务端 denylist 跳过；目标账号独有的键保留。同步前自动备份目标
 * 文件，备份失败零写入；备份可在本卡「备份与还原」里回滚。
 *
 * 第三轮整改（第 12 条）：
 * - 打开卡片即自动预览一次，省掉「先点预览」（按钮旁仍保留状态文案）；
 * - 预览区分「真实变更」与「仅主题归属 / revision 改写」，后者不计入「覆盖 N 项」，
 *   全部目标只剩这类差异时显示「已一致」，不再永远显示「覆盖 1 项」；
 * - 「同步到所有账号」改成二次确认，列清「改几个账号 / 哪些账号的主题会被换掉 /
 *   备份目录」；
 * - 主题两列合并为一列，未登录账号置底并给「去登录」出口。
 */
export function SettingsSyncCard() {
  const [status, setStatus] = useState<SettingsSyncStatus | null>(null);
  const [plan, setPlan] = useState<SettingsSyncPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);

  const [backupsOpen, setBackupsOpen] = useState(false);
  const [backupUid, setBackupUid] = useState<string | null>(null);
  const [backups, setBackups] = useState<SettingsSyncBackupEntry[] | null>(null);
  const [backupsBusy, setBackupsBusy] = useState(false);
  const [backupsError, setBackupsError] = useState<string | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<SettingsSyncBackupEntry | null>(null);

  const loadStatus = useCallback(async () => {
    try {
      setStatus(await api.getSettingsSyncStatus());
      setError(null);
    } catch (cause) {
      setError(api.asError(cause));
    }
  }, []);

  const runPreview = useCallback(async (silent: boolean) => {
    setPreviewing(true);
    try {
      const result = await api.previewSettingsSync();
      setPlan(result);
      setPreviewError(null);
      if (!silent) {
        const changed = result.targets.filter((target) => target.status === "changed");
        toast.info(
          changed.length === 0 ? "目标账号设置已与当前账号一致" : `将修改 ${changed.length} 个账号`,
        );
      }
    } catch (cause) {
      setPlan(null);
      setPreviewError(api.asError(cause));
      if (!silent) toast.error("预览失败", { description: api.asError(cause) });
    } finally {
      setPreviewing(false);
    }
  }, []);

  /** 打开卡片就自动预览一次：用户不必先点「预览差异」才知道能不能同步。 */
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      await loadStatus();
      if (cancelled) return;
      await runPreview(true);
    })();
    return () => {
      cancelled = true;
    };
  }, [loadStatus, runPreview]);

  /** 备份账号默认选中：上次同步的目标（最可能想回滚的），否则第一个本地有目录的账号。 */
  useEffect(() => {
    if (!status || backupUid) return;
    const restored = status.lastResult?.targets.find((target) => target.backup)?.uid;
    const fallback = status.accounts.find((account) => account.hasDirectory)?.uid;
    const next = restored ?? fallback ?? null;
    if (next) setBackupUid(next);
  }, [status, backupUid]);

  const loadBackups = useCallback(async (uid: string) => {
    setBackupsBusy(true);
    try {
      const result = await api.listSettingsSyncBackups(uid);
      setBackups(result.entries);
      setBackupsError(null);
    } catch (cause) {
      setBackups([]);
      setBackupsError(api.asError(cause));
    } finally {
      setBackupsBusy(false);
    }
  }, []);

  useEffect(() => {
    if (!backupsOpen || !backupUid) return;
    void loadBackups(backupUid);
  }, [backupsOpen, backupUid, loadBackups]);

  async function toggle(patch: { enabled?: boolean; autoOnSwitch?: boolean }) {
    setBusy(true);
    try {
      await api.saveSettingsSyncConfig(patch);
      await loadStatus();
    } catch (cause) {
      toast.error("保存失败", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }

  async function apply() {
    setConfirmOpen(false);
    setBusy(true);
    try {
      const result = await api.syncSettings(undefined, false);
      const written = result.targets.filter((target) => target.status === "synced");
      if (result.status === "unchanged" || written.length === 0) {
        toast.info("没有需要写入的账号", { description: "目标设置已与当前账号一致。" });
      } else {
        toast.success(`已同步 ${written.length} 个账号`, {
          description: written[0]?.backup ? `备份：${written[0].backup}` : undefined,
        });
      }
      if (result.status === "partial" || result.status === "failed") {
        const failed = result.targets.filter((target) => target.status === "failed");
        for (const target of failed.slice(0, 3)) {
          toast.error(`${target.label} 同步失败`, { description: target.reason });
        }
      }
      await loadStatus();
      await runPreview(true);
    } catch (cause) {
      toast.error("同步失败（未写入）", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }

  async function restore(entry: SettingsSyncBackupEntry) {
    setRestoreTarget(null);
    if (!backupUid) return;
    setBusy(true);
    try {
      const result = await api.restoreSettingsSyncBackup(backupUid, entry.stamp);
      const copied = result.files.filter((file) => file.action === "restored").length;
      const removed = result.files.filter((file) => file.action === "removed").length;
      toast.success(`已还原 ${formatStamp(entry.stamp, entry.createdAt)} 的备份`, {
        description: `回拷 ${copied} 个文件${removed > 0 ? `，清理 ${removed} 个同步新建的文件` : ""}；还原前备份：${result.preRestoreBackup}`,
      });
      await loadStatus();
      await runPreview(true);
      await loadBackups(backupUid);
    } catch (cause) {
      toast.error("还原失败", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }

  const config = status?.config;
  const accounts = useMemo(() => status?.accounts ?? [], [status]);

  /** 未登录账号置底：本地没有 `storage/user-<uid>-personal` 的排到表格末尾并给「去登录」出口。 */
  const tableRows = useMemo(
    () =>
      [...accounts].sort(
        (left, right) => Number(right.hasDirectory) - Number(left.hasDirectory),
      ),
    [accounts],
  );

  /** 待修改账号数与项数（只看真实变更）。 */
  const changedTargets = plan?.targets.filter((target) => target.status === "changed") ?? [];
  const pendingItems = changedTargets.reduce((sum, target) => sum + target.changes.length, 0);

  /** 这次同步会把哪些账号的主题换掉：确认弹窗里必须点名。 */
  const themeOverwrites = useMemo(() => {
    if (!plan) return [];
    const sourceTheme = accounts.find((account) => account.uid === plan.source.uid)?.theme ?? null;
    const target = themeLabel(sourceTheme);
    return plan.targets
      .filter(
        (item) =>
          item.status === "changed" &&
          item.changes.some((change) => change.key === THEME_KEY && change.action === "overwrite"),
      )
      .map((item) => {
        const change = item.changes.find((entry) => entry.key === THEME_KEY);
        const from = resourceKeyOf(change?.from) ?? "—";
        return { label: item.label, text: `${from} → ${target}` };
      });
  }, [plan, accounts]);

  const previewRows: PreviewRow[] = plan
    ? accounts
        .filter((account) => account.uid !== plan.source.uid)
        .map((account) => ({
          account,
          target: plan.targets.find((target) => target.uid === account.uid) ?? null,
        }))
        .filter((row) => row.account.hasDirectory || row.target !== null)
    : [];

  /** 按钮旁的状态文案：始终告诉用户「现在能不能点同步、为什么」。 */
  const statusText = previewing
    ? "正在预览…"
    : previewError
      ? `预览失败：${previewError}`
      : !plan
        ? "尚未预览"
        : changedTargets.length === 0
          ? "已一致，无需同步"
          : `将修改 ${changedTargets.length} 个账号 · ${pendingItems} 项`;

  const backupAccounts = accounts.filter((account) => account.hasDirectory);

  return (
    <SettingsGroup id="settings-sync" title="设置跨账号同步">
      <CardContent className="space-y-0 p-0">
        <div className="border-b border-border/50 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
          切换账号时把当前账号的界面 / 通用设置（主题、外观资源、权限模式、提示音、语言）同步到目标账号。
          只同步 <code className="font-mono">preferences.json</code>，不同步专家、技能与全局客户端配置；
          <span className="text-foreground/80">目标账号独有的设置项会保留</span>
          ；覆盖前自动备份，备份失败则不写入。
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
              <span className="text-xs tabular-nums text-muted-foreground">
                {accounts.length} 个账号
                <span className={previewError ? "ml-2 text-destructive" : "ml-2 text-foreground/70"}>
                  · {statusText}
                </span>
              </span>
              <div className="flex gap-2">
                <DemoAction>
                  <Button size="sm" variant="ghost" disabled={busy} onClick={() => void loadStatus()}>
                    <RefreshCw className="size-3.5" />
                    刷新
                  </Button>
                </DemoAction>
                <DemoAction>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy || previewing || !config?.enabled}
                    onClick={() => void runPreview(false)}
                  >
                    预览差异
                  </Button>
                </DemoAction>
                <DemoAction>
                  <Button
                    size="sm"
                    disabled={busy || !config?.enabled || !plan || changedTargets.length === 0}
                    onClick={() => setConfirmOpen(true)}
                  >
                    {busy ? <Loader2 className="size-3.5 animate-spin" /> : null}
                    同步到所有账号
                  </Button>
                </DemoAction>
              </div>
            </div>

            {/* 账号属性表：主题合并成一列；未登录账号置底，并给「去登录」出口。 */}
            <div className="max-h-[320px] overflow-y-auto rounded-lg border border-border/60">
              <table className="w-full table-fixed text-xs">
                <colgroup>
                  <col className="w-[44%]" />
                  <col className="w-[30%]" />
                  <col className="w-[26%]" />
                </colgroup>
                <thead className="sticky top-0 z-10 bg-muted text-[11px] text-muted-foreground">
                  <tr>
                    <th className="px-2 py-1.5 text-left font-medium">账号</th>
                    <th className="px-2 py-1.5 text-left font-medium">主题</th>
                    <th className="px-2 py-1.5 text-left font-medium">本机状态</th>
                  </tr>
                </thead>
                <tbody>
                  {tableRows.map((account) => (
                    <tr key={account.uid} className="border-t border-border/40">
                      <td className="truncate px-2 py-1.5" title={`${account.label} · ${account.uid}`}>
                        {account.label}
                      </td>
                      <td className="truncate px-2 py-1.5" title={themeCell(account)}>
                        {themeCell(account)}
                      </td>
                      <td className="px-2 py-1.5">
                        {account.hasDirectory ? (
                          <span
                            className="flex min-w-0 items-baseline gap-1"
                            title={scopesTitle(account.scopes)}
                          >
                            <span className="shrink-0">可同步</span>
                            <span className="min-w-0 truncate text-[10px] text-muted-foreground">
                              · {account.scopes.length} 个工作区配置
                            </span>
                          </span>
                        ) : (
                          <span className="flex min-w-0 items-baseline gap-1 text-muted-foreground">
                            <span className="shrink-0">未登录</span>
                            <Link
                              to="/"
                              className="shrink-0 text-[10px] text-primary underline-offset-2 hover:underline"
                              title="去账号页登录该账号（未在本机登录过的账号没有偏好目录，无法接收同步）"
                            >
                              去登录
                            </Link>
                          </span>
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
                    {changedTargets.length === 0
                      ? "已一致"
                      : `待同步 ${changedTargets.length} 个账号 · ${pendingItems} 项`}
                  </span>
                </div>
                <div className="grid grid-cols-[40%_minmax(0,1.6fr)_auto] items-baseline gap-x-2 gap-y-1">
                  {previewRows.map(({ account, target }) => {
                    const overwrite =
                      target?.changes.filter((change) => change.action === "overwrite").length ?? 0;
                    const added =
                      target?.changes.filter((change) => change.action === "add").length ?? 0;
                    return (
                      <Fragment key={account.uid}>
                        <span className="truncate" title={account.label}>
                          {account.label}
                        </span>
                        <span className="min-w-0">
                          {!target ? (
                            <span className="text-muted-foreground">不在本次计划内</span>
                          ) : target.status === "skipped" ? (
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
                          {target?.themeRewrite && target.status !== "skipped" ? (
                            <Badge
                              variant="secondary"
                              className="text-[10px] font-normal"
                              title="主题的资源与来源相同，只是归属账号 / revision 需要贴到目标账号；不写入时会保持原样"
                            >
                              {target.status === "changed" ? "主题归属改写" : "仅主题归属差异"}
                            </Badge>
                          ) : null}
                        </span>
                      </Fragment>
                    );
                  })}
                </div>
                <p className="mt-1.5 text-[10px] leading-4 text-muted-foreground/80">
                  源账号与未登录账号不参与同步，因此这里的行数比上表少。「仅主题归属差异」不计入项数，也不会写入。
                </p>
              </div>
            ) : (
              <p className="mt-3 text-[11px] text-muted-foreground">
                {previewing
                  ? "正在自动预览…"
                  : previewError
                    ? `自动预览失败：${previewError}`
                    : "点「预览差异」查看将变更的内容。"}
              </p>
            )}

            {/* 备份与还原：备份落在 ~/.workbuddy-shift/backups/settings/<uid>/<stamp>/。 */}
            <div className="mt-3 rounded-lg border border-border/60 text-[11px]">
              <button
                type="button"
                className="flex w-full items-center gap-1.5 px-2.5 py-2 text-left font-medium text-foreground hover:bg-foreground/[0.03]"
                onClick={() => setBackupsOpen((open) => !open)}
                aria-expanded={backupsOpen}
              >
                {backupsOpen ? (
                  <ChevronDown className="size-3.5 shrink-0" />
                ) : (
                  <ChevronRight className="size-3.5 shrink-0" />
                )}
                备份与还原
                <span className="ml-1 font-normal text-muted-foreground">
                  · 同步前的目标设置都在本机留了一份，可随时还原
                </span>
              </button>
              {backupsOpen ? (
                <div className="space-y-1.5 border-t border-border/50 px-2.5 py-2">
                  {backupAccounts.length === 0 ? (
                    <p className="text-muted-foreground">还没有可还原的账号（目标账号需先在本机登录过）。</p>
                  ) : (
                    <>
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-muted-foreground">账号</span>
                        <Select
                          value={backupUid ?? undefined}
                          onValueChange={(value) => {
                            setBackupUid(value);
                            setBackups(null);
                          }}
                        >
                          <SelectTrigger
                            size="sm"
                            className="w-52 bg-background text-xs"
                            aria-label="选择要还原的账号"
                          >
                            <SelectValue placeholder="选择账号" />
                          </SelectTrigger>
                          <SelectContent>
                            {backupAccounts.map((account) => (
                              <SelectItem key={account.uid} value={account.uid}>
                                {account.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={backupsBusy || !backupUid}
                          onClick={() => backupUid && void loadBackups(backupUid)}
                        >
                          {backupsBusy ? <Loader2 className="size-3.5 animate-spin" /> : null}
                          刷新备份
                        </Button>
                      </div>
                      {backupsError ? (
                        <p className="text-destructive">{backupsError}</p>
                      ) : backups === null ? (
                        <p className="text-muted-foreground">正在读取备份…</p>
                      ) : backups.length === 0 ? (
                        <p className="text-muted-foreground">该账号没有备份（还没同步过）。</p>
                      ) : (
                        <ul className="divide-y divide-border/40">
                          {backups.map((entry) => (
                            <li
                              key={entry.stamp}
                              className="flex flex-wrap items-center justify-between gap-2 py-1.5"
                            >
                              <div className="min-w-0">
                                <div className="tabular-nums">
                                  {formatStamp(entry.stamp, entry.createdAt)}
                                  <span className="ml-1.5 font-mono text-[10px] text-muted-foreground">
                                    {entry.stamp}
                                  </span>
                                </div>
                                <div className="truncate text-[10px] text-muted-foreground" title={entry.path}>
                                  {entry.files.length} 个文件
                                  {entry.hasManifest ? "" : " · 清单缺失，可能无法还原"}
                                  <span className="ml-1 font-mono">{entry.path}</span>
                                </div>
                              </div>
                              <DemoAction>
                                <Button
                                  size="sm"
                                  variant="outline"
                                  disabled={busy || !entry.hasManifest}
                                  onClick={() => setRestoreTarget(entry)}
                                >
                                  还原
                                </Button>
                              </DemoAction>
                            </li>
                          ))}
                        </ul>
                      )}
                      <p className="text-[10px] leading-4 text-muted-foreground/80">
                        还原会覆盖该账号当前的 preferences.json（同步时新建的文件会被清理）；还原前会自动再备份一份当前文件。
                      </p>
                    </>
                  )}
                </div>
              ) : null}
            </div>

            <div className="mt-3 border-t border-border/60 pt-2.5 text-[11px] text-muted-foreground">
              <div className="mb-1 text-[11px] font-medium text-foreground">上次结果</div>
              {status.lastResult ? (
                <>
                  <div>
                    最近一次：{resultSummary(status.lastResult)} ·{" "}
                    {new Date(status.lastResult.at).toLocaleString("zh-CN")} · 备份根{" "}
                    <span className="font-mono">{status.lastResult.backupRoot}</span>
                  </div>
                  {status.lastResult.targets.slice(0, 3).map((target) => (
                    <div key={target.uid} className="break-all">
                      {target.label}：
                      {target.status === "skipped"
                        ? `跳过（${target.reason}）`
                        : target.status === "failed"
                          ? `失败（${target.reason}）`
                          : target.status === "unchanged"
                            ? "已一致"
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

      {/* 二次确认：写 10 个账号是不可撤销操作（只能事后还原备份），必须点名影响面。 */}
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              同步设置到 {changedTargets.length} 个账号？
            </AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className="space-y-1.5 text-left text-xs leading-5">
                <div>
                  将修改{" "}
                  <span className="font-medium text-foreground tabular-nums">
                    {changedTargets.length} 个账号
                  </span>
                  ，共 {pendingItems} 项设置。
                </div>
                {themeOverwrites.length > 0 ? (
                  <div>
                    将覆盖主题的账号：
                    <span className="text-foreground">
                      {themeOverwrites
                        .slice(0, 5)
                        .map((item) => `${item.label}（${item.text}）`)
                        .join("、")}
                      {themeOverwrites.length > 5
                        ? ` 等 ${themeOverwrites.length} 个账号`
                        : ""}
                    </span>
                  </div>
                ) : (
                  <div>本次不改写任何账号的主题。</div>
                )}
                <div>
                  备份目录：
                  <span className="font-mono break-all">{plan?.backupRoot ?? "—"}</span>
                </div>
                <div>目标账号独有的设置项会保留；写错可在「备份与还原」里回滚。</div>
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={() => void apply()}>确认同步</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* 还原确认：覆盖当前设置，同样需要点名账号与备份时间。 */}
      <AlertDialog
        open={restoreTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRestoreTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>还原这次备份？</AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className="space-y-1.5 text-left text-xs leading-5">
                <div>
                  账号：
                  <span className="text-foreground">
                    {accounts.find((account) => account.uid === backupUid)?.label ?? backupUid}
                  </span>
                </div>
                <div>
                  备份时间：
                  <span className="text-foreground tabular-nums">
                    {restoreTarget
                      ? formatStamp(restoreTarget.stamp, restoreTarget.createdAt)
                      : "—"}
                  </span>
                </div>
                <div>
                  将回拷{" "}
                  <span className="text-foreground tabular-nums">
                    {restoreTarget?.files.filter((file) => file.existed).length ?? 0}
                  </span>{" "}
                  个文件，覆盖该账号当前的设置；还原前会先把当前文件再备份一份。
                </div>
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => restoreTarget && void restore(restoreTarget)}
            >
              确认还原
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </SettingsGroup>
  );
}
