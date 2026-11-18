import { useEffect, useState } from "react";
import { CircleCheck, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { Accordion } from "@/components/ui/accordion";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { TimePicker } from "@/components/ui/time-picker";
import { displayName } from "@/lib/account-display";
import * as api from "@/lib/api";
import type {
  AccountMeta,
  CheckinConfig,
  CheckinLog,
} from "@/lib/types";

import { cn } from "@/lib/utils";
import { accountVariant, variantSupportsCheckin, variantSupportsTravel } from "@/lib/variant";
import { DemoAction } from "@/components/demo-action";
import { useAccountsStore } from "@/stores/accounts";
import { AutoTravelRow } from "./auto-travel-row";
import { CHECKIN_NUMBER_FIELDS, CheckinNumberKey, checkinWindowIssue, formatTime, logLabel } from "./helpers";
import { AccordionSettingsRow, INSET_PANEL, NumberSettingRow, PANEL_ROW, SettingsFieldRow, SettingsGroup } from "./rows";
import { useConfigAutosave } from "./use-config-autosave";

/** 自动签到配置 + 一键签到 + 日志（含自动旅行行）。 */
export function AutoCheckinCard() {
  /** 自动旅行与自动签到同卡，按档位决定是否渲染该行（国际版无成长中心）。 */
  const variant = useAccountsStore((s) => s.variant);
  const accounts = useAccountsStore((s) => s.accounts);
  const fetchAll = useAccountsStore((s) => s.fetchAll);
  const autosave = useConfigAutosave<CheckinNumberKey, CheckinConfig>({
    save: api.saveAutoCheckinConfig,
    saveErrorMessage: "自动签到设置保存失败",
    numberFields: CHECKIN_NUMBER_FIELDS,
    enabledSuccessTitles: { on: "自动签到已开启", off: "自动签到已关闭" },
    // 落盘值回填显示：保留半填 / 非法时间段草稿，那是刻意不落盘的本地状态。
    mergeAdopted: (saved, draft) =>
      draft && checkinWindowIssue(draft.checkin_start, draft.checkin_end) !== null
        ? { ...saved, checkin_start: draft.checkin_start, checkin_end: draft.checkin_end }
        : saved,
  });
  const { cfg } = autosave;
  /**
   * 手风琴展开的面板：参数区 / 逐账号开关 / 签到日志。
   *
   * 用 multiple 而非 single：三块内容彼此独立，用户可能同时对照参数与账号名单。
   */
  const [openSections, setOpenSections] = useState<string[]>([]);
  const logsOpen = openSections.includes("logs");
  const [logs, setLogs] = useState<CheckinLog[] | null>(null);
  const [logsError, setLogsError] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void loadConfig();
  }, []);

  // 账号列表来自全局 store：为空时补拉一次（只取状态与账号，不拉积分）。
  useEffect(() => {
    if (accounts.length === 0) void fetchAll();
  }, [accounts.length, fetchAll]);

  // 日志懒加载：展开时才请求，每次展开重新拉取；收起态无可见列表，不请求。
  useEffect(() => {
    if (!logsOpen) return;
    let cancelled = false;
    void loadLogs(() => cancelled);
    return () => {
      cancelled = true;
    };
  }, [logsOpen]);

  async function loadConfig() {
    try {
      autosave.initialize(await api.getAutoCheckinConfig());
    } catch (e) {
      toast.error("自动签到配置加载失败", { description: api.asError(e) });
    }
  }

  /**
   * 时间段：先只改本地显示；仅「两端都填且 start < end」或「两端都清空」才落盘。
   * 半填 / 非法组合不写盘（后端把这种窗口当「不限制」，写盘会让下一轮对全部账号发起签到）。
   */
  function onWindowChange(key: "checkin_start" | "checkin_end", value: string) {
    const current = autosave.draft();
    const saved = autosave.saved();
    if (!current || !saved) return;
    const next = { ...current };
    next[key] = value;
    autosave.updateDraft(next);
    if (checkinWindowIssue(next.checkin_start, next.checkin_end) !== null) return;
    if (next.checkin_start === saved.checkin_start && next.checkin_end === saved.checkin_end) return;
    autosave.enqueueSave(
      { checkin_start: next.checkin_start, checkin_end: next.checkin_end },
      next.checkin_start
        ? { title: "签到时间段已保存", description: `${next.checkin_start} 至 ${next.checkin_end}` }
        : { title: "已清除签到时间段限制" },
    );
  }

  /** 「清除」按钮：两端清空属完整语义，立即落盘。 */
  function clearWindow() {
    const current = autosave.draft();
    const saved = autosave.saved();
    if (!current || !saved) return;
    autosave.updateDraft({ ...current, checkin_start: "", checkin_end: "" });
    if (!saved.checkin_start && !saved.checkin_end) return;
    autosave.enqueueSave(
      { checkin_start: "", checkin_end: "" },
      { title: "已清除签到时间段限制" },
    );
  }

  async function loadLogs(isCancelled?: () => boolean) {
    setLogsError("");
    try {
      const res = await api.getCheckinLogs();
      if (!isCancelled?.()) setLogs(res.logs);
    } catch (e) {
      if (isCancelled?.()) return;
      setLogs(null);
      setLogsError(api.asError(e));
    }
  }

  async function checkinAllNow() {
    if (busy || autosave.saving) return;
    setBusy(true);
    try {
      const res = await api.checkinAll();
      if (res.status === "skipped" && res.reason === "already_running") {
        toast.error("签到任务正在进行，请稍后再试");
        return;
      }
      const ok = res.accounts.filter((a) => a.result === "success").length;
      const already = res.accounts.filter((a) => a.result === "already").length;
      const err = res.accounts.filter((a) => a.result === "error").length;
      const skipped = res.accounts.filter(
        (a) => a.result === "skipped" && a.reason === "auto_checkin_disabled",
      ).length;
      const detail = res.accounts
        .filter((a) => a.result === "error")
        .map((a) => `${a.email}（${a.error}）`)
        .join("；");
      const text = `签到完成：成功 ${ok}，已签 ${already}，失败 ${err}，已忽略 ${skipped} 个关闭自动签到的账号${detail ? `。${detail}` : ""}`;
      const allFailed = res.accounts.length > 0 && err === res.accounts.length;
      const allSkippedOrUnavailable =
        res.accounts.length === 0 || (ok === 0 && already === 0 && err === 0);
      if (allFailed) toast.error(text);
      else if (allSkippedOrUnavailable) toast.info(text);
      else toast.success(text);
      void loadConfig();
      if (logsOpen) void loadLogs();
    } catch (e) {
      toast.error("签到失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  /**
   * 逐账号开关：切换即落盘，提交最新一份名单。
   *
   * 与参数编辑共用同一条提交链，名单从当前显示草稿派生（连续切换不会互相覆盖），
   * 失败由提交链统一回滚并提示。
   */
  function onAutoCheckinChange(account: AccountMeta, allowed: boolean) {
    const current = autosave.draft();
    if (!current) return;
    const excluded = new Set(current.excluded_account_ids ?? []);
    if (allowed) excluded.delete(account.id);
    else excluded.add(account.id);
    const next = [...excluded];
    autosave.updateDraft({ ...current, excluded_account_ids: next });
    autosave.enqueueSave({ excluded_account_ids: next });
  }

  /** 逐账号开关只列出支持签到的档位（国际版签到接口未开放）。 */
  const checkinAccounts = accounts.filter((account) => variantSupportsCheckin(accountVariant(account)));
  const excludedIds = new Set(cfg?.excluded_account_ids ?? []);
  const windowIssue = cfg ? checkinWindowIssue(cfg.checkin_start, cfg.checkin_end) : null;

  return (
    <SettingsGroup
      id="settings-auto-checkin"
      title="自动签到"
    >
      <CardContent className="space-y-0 p-0">
        <Accordion type="multiple" value={openSections} onValueChange={setOpenSections}>
          {cfg ? (
            <>
              <AccordionSettingsRow
                value="params"
                label="启用自动签到"
                description="为允许自动签到的账号核验状态并补签；可在下方按账号关闭"
                actions={
                  <>
                    <DemoAction>
                      <Switch
                        aria-label="启用自动签到"
                        checked={cfg.enabled}
                        onCheckedChange={autosave.onToggleEnabled}
                      />
                    </DemoAction>
                    <DemoAction>
                      <Button size="sm" variant="outline" onClick={checkinAllNow} disabled={busy || autosave.saving}>
                        {busy ? <Loader2 className="animate-spin" /> : <CircleCheck />}全部立即签到
                      </Button>
                    </DemoAction>
                  </>
                }
              >
                <div className={INSET_PANEL}>
                  <SettingsFieldRow
                    className={PANEL_ROW}
                    label="签到时间段"
                    description={
                      <>
                        留空为不限制。设置后每天在窗口内随机时刻自动签到。
                        <span className="mt-0.5 block">需 App 在窗口附近运行才能按时执行。</span>
                      </>
                    }
                  >
                    <div className="flex min-w-0 w-full flex-col items-end gap-1 sm:w-auto">
                      <div className="flex min-w-0 w-full flex-wrap items-center justify-end gap-2 sm:w-auto">
                        <DemoAction className="min-w-0 flex-1 sm:flex-none">
                          <TimePicker
                            className="min-w-0 flex-1 sm:flex-none"
                            value={cfg.checkin_start}
                            hourLabel="签到开始时间（小时）"
                            minuteLabel="签到开始时间（分钟）"
                            onChange={(v) => onWindowChange("checkin_start", v)}
                          />
                        </DemoAction>
                        <span className="shrink-0 text-xs text-muted-foreground">至</span>
                        <DemoAction className="min-w-0 flex-1 sm:flex-none">
                          <TimePicker
                            className="min-w-0 flex-1 sm:flex-none"
                            value={cfg.checkin_end}
                            hourLabel="签到结束时间（小时）"
                            minuteLabel="签到结束时间（分钟）"
                            onChange={(v) => onWindowChange("checkin_end", v)}
                          />
                        </DemoAction>
                        {(cfg.checkin_start || cfg.checkin_end) && (
                          <DemoAction>
                            <Button
                              size="sm"
                              variant="ghost"
                              className="shrink-0"
                              onClick={clearWindow}
                            >
                              清除
                            </Button>
                          </DemoAction>
                        )}
                      </div>
                      {windowIssue && (
                        <p className="text-xs leading-4 text-warning">{windowIssue}</p>
                      )}
                    </div>
                  </SettingsFieldRow>

                  <NumberSettingRow
                    className={PANEL_ROW}
                    id="ac-keep"
                    spec={CHECKIN_NUMBER_FIELDS.keepalive_days}
                    description="天；0 表示每天无条件刷新"
                    value={autosave.numDraft.keepalive_days ?? String(cfg.keepalive_days)}
                    onChange={(text) => autosave.onNumberChange("keepalive_days", text)}
                    onCommit={(raw) => autosave.onNumberCommit("keepalive_days", raw)}
                  />
                  <NumberSettingRow
                    className={cn(PANEL_ROW, "border-b-0")}
                    id="ac-lazy"
                    spec={CHECKIN_NUMBER_FIELDS.lazy_refresh_hours}
                    description="小时"
                    value={autosave.numDraft.lazy_refresh_hours ?? String(cfg.lazy_refresh_hours)}
                    onChange={(text) => autosave.onNumberChange("lazy_refresh_hours", text)}
                    onCommit={(raw) => autosave.onNumberCommit("lazy_refresh_hours", raw)}
                  />
                </div>
              </AccordionSettingsRow>

              {/* 逐账号开关：切换即落盘，与参数编辑共用同一条提交链。 */}
              <AccordionSettingsRow
                value="excluded"
                label="不参与自动签到的账号"
                description="关闭后，后台轮次、刷新积分与全部立即签到都会跳过该账号；仍可在账号卡片进行单账号手动签到。"
              >
                <div className={INSET_PANEL}>
                  {checkinAccounts.length === 0 ? (
                    <p className="py-2 text-xs text-muted-foreground">暂无可签到的账号</p>
                  ) : (
                    checkinAccounts.map((account) => {
                      const name = displayName(account);
                      return (
                        <div
                          key={account.id}
                          className="flex min-w-0 items-center justify-between gap-3 border-b border-border/50 py-2.5 last:border-b-0"
                        >
                          <span className="min-w-0 flex-1 truncate text-xs leading-4" title={name}>
                            {name}
                          </span>
                          <DemoAction>
                            <Switch
                              checked={!excludedIds.has(account.id)}
                              onCheckedChange={(allowed) => onAutoCheckinChange(account, allowed)}
                              aria-label={`${name}参与自动签到`}
                            />
                          </DemoAction>
                        </div>
                      );
                    })
                  )}
                </div>
              </AccordionSettingsRow>
            </>
          ) : (
            <p className="px-4 py-3 text-sm text-muted-foreground sm:px-5">加载配置中…</p>
          )}

          {/* 成长中心（派猫猫旅行）仅国内版开放：国际版不渲染该行，也不请求其配置。
              行本身独立于签到配置的加载状态，签到配置读取失败也不影响开关。 */}
          {variantSupportsTravel(variant) ? <AutoTravelRow /> : null}

          <AccordionSettingsRow
            value="logs"
            label="签到日志"
            description="保留最近 30 天；本机明文保存，可能含账号昵称。"
            divider={false}
          >
            <div className={INSET_PANEL}>
              {logsError ? (
                <p className="py-2 text-xs text-danger">{logsError}</p>
              ) : !logs ? (
                <p className="py-2 text-xs text-muted-foreground">正在读取…</p>
              ) : logs.length === 0 ? (
                <p className="py-2 text-xs text-muted-foreground">暂无签到记录</p>
              ) : (
                <div className="max-h-64 overflow-y-auto pr-1">
                  {[...logs].reverse().map((l, i) => {
                    const tone = logLabel(l.result);
                    return (
                      <div
                        key={i}
                        className="flex items-center justify-between border-b border-border/50 py-2 text-xs last:border-b-0"
                      >
                        <div className="min-w-0 flex-1 truncate">
                          <span className="font-medium">{l.email}</span>
                          {l.error && <span className="text-danger">（{l.error}）</span>}
                        </div>
                        <div className="ml-2 flex shrink-0 items-center gap-2">
                          <span
                            className={
                              tone.tone === "error"
                                ? "text-danger"
                                : tone.tone === "warning"
                                  ? "text-warning"
                                  : "text-success"
                            }
                          >
                            {tone.text}
                          </span>
                          <span className="text-muted-foreground">{formatTime(l.ts)}</span>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </AccordionSettingsRow>
        </Accordion>
      </CardContent>
    </SettingsGroup>
  );
}
