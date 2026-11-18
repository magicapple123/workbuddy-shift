import { useEffect, useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Accordion } from "@/components/ui/accordion";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import type {
  AutoRotateConfig,
  RotateLog,
  RotateStatus,
} from "@/lib/types";

import { cn } from "@/lib/utils";
import { DemoAction } from "@/components/demo-action";
import { ROTATE_NUMBER_FIELDS, RotateNumberKey, formatTime } from "./helpers";
import { AccordionSettingsRow, INSET_PANEL, NumberSettingRow, PANEL_ROW, SettingsGroup } from "./rows";
import { useConfigAutosave } from "./use-config-autosave";

/** 自动轮换配置（CodeBuddy CLI）+ 手动检查 + 日志。 */
export function AutoRotateCard() {
  const autosave = useConfigAutosave<RotateNumberKey, AutoRotateConfig>({
    save: api.saveAutoRotateConfig,
    saveErrorMessage: "自动轮换设置保存失败",
    numberFields: ROTATE_NUMBER_FIELDS,
    enabledSuccessTitles: { on: "自动轮换已开启", off: "自动轮换已关闭" },
  });
  const { cfg } = autosave;
  const [status, setStatus] = useState<RotateStatus | null>(null);
  /** 手风琴展开的面板：参数区 / 轮换日志，默认全部收起；开关行与操作行常驻。 */
  const [openSections, setOpenSections] = useState<string[]>([]);
  const logsOpen = openSections.includes("logs");
  const [logs, setLogs] = useState<RotateLog[] | null>(null);
  const [logsError, setLogsError] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void loadConfig();
  }, []);

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
      const [c, s] = await Promise.all([api.getAutoRotateConfig(), api.getRotateStatus()]);
      autosave.initialize(c);
      setStatus(s);
    } catch (e) {
      toast.error("自动轮换配置加载失败", { description: api.asError(e) });
    }
  }

  async function loadLogs(isCancelled?: () => boolean) {
    setLogsError("");
    try {
      const res = await api.getRotateLogs();
      if (!isCancelled?.()) setLogs(res.logs);
    } catch (e) {
      if (isCancelled?.()) return;
      setLogs(null);
      setLogsError(api.asError(e));
    }
  }

  async function runNow() {
    setBusy(true);
    try {
      const res = await api.runRotate();
      // webui 没有事件通道：手动检查的推迟提示只能从返回值里取（桌面端由
      // rotate-deferred 事件统一弹出，避免同一件事弹两次）。
      if (api.isWebui() && res.notify?.body) {
        toast.warning("自动轮换已推迟", { description: res.notify.body, duration: 10_000 });
      }
      const text =
        res.status === "switched"
          ? `已切换到 ${res.to ?? "目标账号"}`
          : res.status === "disabled"
            ? "自动轮换未启用（请在下方开启后重试）"
            : (res.reason ?? `检查完成：${res.status}`);
      if (res.status === "error") toast.error(text);
      else toast.success(text);
      void loadConfig();
      if (logsOpen) void loadLogs();
    } catch (e) {
      toast.error("轮换检查失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  function actionLabel(action: string): { text: string; tone: "success" | "warning" | "error" } {
    switch (action) {
      case "switched":
        return { text: "已切换", tone: "success" };
      case "skipped":
        return { text: "未切换", tone: "warning" };
      case "disabled":
        return { text: "未启用", tone: "warning" };
      case "error":
        return { text: "出错", tone: "error" };
      default:
        return { text: action, tone: "warning" };
    }
  }

  return (
    <SettingsGroup
      id="settings-auto-rotate"
      title="CodeBuddy CLI 自动轮换"
    >
      <CardContent className="space-y-0 p-0">
        {status && (
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-border/60 bg-muted/25 px-4 py-3 text-xs text-muted-foreground sm:px-5">
            <span>
              当前 CLI 账号：
              <b className="text-foreground">{status.activeAccountName ?? "未配置"}</b>
            </span>
            {status.lastCheckAt && <span>上次检查 {formatTime(status.lastCheckAt)}</span>}
            {status.lastSwitchAt && <span>上次切换 {formatTime(status.lastSwitchAt)}</span>}
            {!status.cliConfigured && (
              <span className="text-danger">未接入 CodeBuddy CLI（请先到账号页安装 helper）</span>
            )}
          </div>
        )}

        <Accordion type="multiple" value={openSections} onValueChange={setOpenSections}>
        {cfg ? (
          <>
            <AccordionSettingsRow
              value="params"
              label="启用自动轮换"
              description="开启后按设定的间隔自动检查并切换 CodeBuddy CLI 账号"
              actions={
                <>
                  <DemoAction>
                    <Switch
                      aria-label="启用自动轮换"
                      checked={cfg.enabled}
                      onCheckedChange={autosave.onToggleEnabled}
                    />
                  </DemoAction>
                  <DemoAction>
                    <Button size="sm" variant="outline" onClick={runNow} disabled={busy}>
                      {busy ? <Loader2 className="animate-spin" /> : <RefreshCw />}立即检查一次
                    </Button>
                  </DemoAction>
                </>
              }
            >
              <div className={INSET_PANEL}>
                <NumberSettingRow
                  className={PANEL_ROW}
                  id="ar-interval"
                  spec={ROTATE_NUMBER_FIELDS.check_interval_minutes}
                  description="分钟"
                  value={autosave.numDraft.check_interval_minutes ?? String(cfg.check_interval_minutes)}
                  onChange={(text) => autosave.onNumberChange("check_interval_minutes", text)}
                  onCommit={(raw) => autosave.onNumberCommit("check_interval_minutes", raw)}
                />
                <NumberSettingRow
                  className={PANEL_ROW}
                  id="ar-cooldown"
                  spec={ROTATE_NUMBER_FIELDS.cooldown_minutes}
                  description="分钟"
                  value={autosave.numDraft.cooldown_minutes ?? String(cfg.cooldown_minutes)}
                  onChange={(text) => autosave.onNumberChange("cooldown_minutes", text)}
                  onCommit={(raw) => autosave.onNumberCommit("cooldown_minutes", raw)}
                />
                <NumberSettingRow
                  className={PANEL_ROW}
                  id="ar-gap"
                  spec={ROTATE_NUMBER_FIELDS.min_gap_hours}
                  description="小时"
                  value={autosave.numDraft.min_gap_hours ?? String(cfg.min_gap_hours)}
                  onChange={(text) => autosave.onNumberChange("min_gap_hours", text)}
                  onCommit={(raw) => autosave.onNumberCommit("min_gap_hours", raw)}
                />
                <NumberSettingRow
                  className={PANEL_ROW}
                  id="ar-urgency"
                  spec={ROTATE_NUMBER_FIELDS.min_urgency_hours}
                  description="小时"
                  value={autosave.numDraft.min_urgency_hours ?? String(cfg.min_urgency_hours)}
                  onChange={(text) => autosave.onNumberChange("min_urgency_hours", text)}
                  onCommit={(raw) => autosave.onNumberCommit("min_urgency_hours", raw)}
                />
                <NumberSettingRow
                  className={cn(PANEL_ROW, "border-b-0")}
                  id="ar-min"
                  spec={ROTATE_NUMBER_FIELDS.min_remaining_credits}
                  description="低于此值时不切换"
                  value={autosave.numDraft.min_remaining_credits ?? String(cfg.min_remaining_credits)}
                  onChange={(text) => autosave.onNumberChange("min_remaining_credits", text)}
                  onCommit={(raw) => autosave.onNumberCommit("min_remaining_credits", raw)}
                />
                <p className="pt-1 text-[13px] leading-5 text-muted-foreground">
                  切换时机：目标账号剩余到期时间少于「紧迫阈值」且比当前账号早超过「差异阈值」，且目标剩余积分不低于「最小剩余积分」。检测到有 CodeBuddy CLI 会话在运行时，本次轮换会跳过并在当日最多提示 5 次；重启 CLI 后新账号才会生效。
                </p>
              </div>
            </AccordionSettingsRow>
          </>
        ) : (
          <p className="px-4 py-3 text-sm text-muted-foreground sm:px-5">加载配置中…</p>
        )}

        <AccordionSettingsRow
          value="logs"
          label="轮换日志"
          description="保留最近 200 条；本机明文保存，可能含账号昵称。"
          divider={false}
        >
          <div className={INSET_PANEL}>
            {logsError ? (
              <p className="py-2 text-xs text-danger">{logsError}</p>
            ) : !logs ? (
              <p className="py-2 text-xs text-muted-foreground">正在读取…</p>
            ) : logs.length === 0 ? (
              <p className="py-2 text-xs text-muted-foreground">暂无轮换记录</p>
            ) : (
              <div className="max-h-64 overflow-y-auto pr-1">
                {logs.map((l, i) => {
                  const tone = actionLabel(l.action);
                  return (
                    <div
                      key={i}
                      className="flex items-center justify-between border-b border-border/50 py-2 text-xs last:border-b-0"
                    >
                      <div className="min-w-0 flex-1 truncate">
                        {l.action === "switched" && l.from && l.to && (
                          <span className="font-medium">
                            {l.from.name ?? l.from.id} → {l.to.name ?? l.to.id}
                          </span>
                        )}
                        {l.reason && <span className="text-muted-foreground">（{l.reason}）</span>}
                      </div>
                      <div className="ml-2 flex shrink-0 items-center gap-2">
                        <span
                          className={
                            tone.tone === "error"
                              ? "text-danger"
                              : tone.tone === "success"
                                ? "text-success"
                                : "text-warning"
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
