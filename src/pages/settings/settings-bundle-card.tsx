import { useRef, useState, type ChangeEvent } from "react";
import { Download, Loader2, Upload } from "lucide-react";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import * as api from "@/lib/api";
import type { SettingsBundle, SettingsBundleKey } from "@/lib/types";

import { SettingsFieldRow, SettingsGroup } from "./rows";

/** 导入结果里的键 → 用户看得懂的配置名（顺序无关，报告本身有序）。 */
const BUNDLE_KEY_LABELS: Record<SettingsBundleKey, string> = {
  checkin: "自动签到",
  travel: "派猫猫旅行",
  rotate: "自动轮换",
  rateLimit: "限额监听",
};

/** 导出文件名：workbuddy-shift-settings-YYYY-MM-DD.json */
function bundleFileName(): string {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  return `workbuddy-shift-settings-${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}.json`;
}

/**
 * 触发浏览器下载。
 *
 * 桌面端（WebView2 / WKWebView）与 WebUI 用的是同一段代码：`export_accounts` 那条
 * 路径之所以走「保存对话框 + 后端写盘」，是因为账号备份含 token、必须落到用户选定的
 * 绝对路径；设置包是纯偏好、不含任何密钥，走浏览器下载更简单，也不需要新增落盘命令。
 * 已知限制：macOS WKWebView 对 `<a download>` 支持不完整，届时用户可从浏览器下载目录
 * 找不到文件——要彻底解决需要新增「保存设置包到路径」的命令（不在本次改动范围）。
 */
function downloadJson(filename: string, data: unknown): void {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  document.body.removeChild(anchor);
  URL.revokeObjectURL(url);
}

/** JSON 对象判定（数组与 null 都不算）。 */
function isJsonObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

interface Props {
  /** 导入成功后的回调：页面据此重挂载各配置卡（它们只在挂载时读一次配置）。 */
  onImported?: () => void;
}

/**
 * 设置包：把四份自动化配置（签到 / 旅行 / 轮换 / 限额）导出成一个 JSON，或反之导入。
 *
 * 不含账号库（带 token）与更新配置（可能带代理口令），因此这份文件可以安全地
 * 放在网盘、发给同事、跨机器搬运。导出只能读、导入写成败都由 core 的白名单校验决定。
 */
export function SettingsBundleCard({ onImported }: Props) {
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);

  async function onExport() {
    if (exporting) return;
    setExporting(true);
    try {
      const bundle: SettingsBundle = await api.exportSettingsBundle();
      downloadJson(bundleFileName(), bundle);
      toast.success("已导出设置包", {
        description: "包含自动签到、旅行、自动轮换与限额监听四份配置；不含账号与更新配置",
      });
    } catch (e) {
      toast.error("导出设置包失败", { description: api.asError(e) });
    } finally {
      setExporting(false);
    }
  }

  async function onFilePicked(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    // 立刻清空 input：同一个文件连续选两次也要能触发 change。
    event.target.value = "";
    if (!file || importing) return;
    setImporting(true);
    try {
      const text = await file.text();
      let parsed: unknown;
      try {
        parsed = JSON.parse(text);
      } catch {
        toast.error("导入失败", { description: "文件不是合法的 JSON" });
        return;
      }
      if (!isJsonObject(parsed)) {
        toast.error("导入失败", { description: "设置包必须是 JSON 对象" });
        return;
      }
      const report = await api.importSettingsBundle(parsed);
      const names = report.imported.map((key) => BUNDLE_KEY_LABELS[key] ?? key).join("、");
      toast.success("已导入设置包", {
        description: `${names}；原配置已备份到 ${report.backupDir}`,
      });
      onImported?.();
    } catch (e) {
      toast.error("导入设置包失败", { description: api.asError(e) });
    } finally {
      setImporting(false);
    }
  }

  return (
    <SettingsGroup id="settings-bundle" title="设置包">
      <CardContent className="space-y-0 p-0">
        <div className="border-b border-border/50 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
          换机、重装或备份用：把自动签到、派猫猫旅行、自动轮换与限额监听四份配置打包成一个 JSON。
          <span className="text-foreground/80">不含账号（带登录凭据）与自动更新配置（可能带代理口令）</span>
          ；导入前会自动备份现有配置，任一字段不合法则整体拒绝、不写入。
        </div>

        <SettingsFieldRow
          label="导出设置包"
          description="下载一个 JSON 文件，可保存或复制到其它机器"
        >
          <DemoAction>
            <Button size="sm" variant="outline" disabled={exporting} onClick={() => void onExport()}>
              {exporting ? <Loader2 className="animate-spin" /> : <Download />}
              导出
            </Button>
          </DemoAction>
        </SettingsFieldRow>

        <SettingsFieldRow
          className="border-b-0"
          label="导入设置包"
          description="从导出的 JSON 恢复这四份配置；不会改动账号库"
        >
          <DemoAction>
            <Button
              size="sm"
              variant="outline"
              disabled={importing}
              onClick={() => fileInputRef.current?.click()}
            >
              {importing ? <Loader2 className="animate-spin" /> : <Upload />}
              选择文件
            </Button>
          </DemoAction>
          <input
            ref={fileInputRef}
            type="file"
            accept=".json,application/json"
            className="hidden"
            aria-label="选择设置包文件"
            onChange={(event) => void onFilePicked(event)}
          />
        </SettingsFieldRow>
      </CardContent>
    </SettingsGroup>
  );
}
