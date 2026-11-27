import { useState } from "react";
import { Check, Copy, Download, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import * as api from "@/lib/api";
import { downloadTextFile } from "@/lib/export-download";
import type { DiagnosticsReport } from "@/lib/types";
import { SettingsGroup } from "./rows";

/** 导出文件名：带生成时间，用户连导两次不会互相覆盖。 */
function fileNameFor(report: DiagnosticsReport): string {
  const stamp = new Date(report.generatedAt)
    .toISOString()
    .replace(/[:T]/g, "-")
    .slice(0, 16);
  return `workbuddy-shift-diagnostics-${stamp}.txt`;
}

/** 体积提示：报告是纯文本，几百 KB 已经算很大（日志最多 200 行）。 */
function sizeLabel(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

/**
 * 诊断包：一键得到「能直接贴进 issue 的脱敏信息」。
 *
 * 用户来反馈时往往说不出关键事实（版本、宿主、哪个客户端没装、日志最后一条），
 * 作者要来回问三轮。这里把那一组事实交给后端一次装配并**自动脱敏**
 * （路径 / UUID / URL 路径 / 邮箱 / 令牌 / 用户名 / 长数字），前端只负责展示与搬运。
 *
 * 两个动作：
 * - **预览诊断包**：弹层显示将要导出的全文（可滚动），可一键复制；
 * - **导出诊断包**：桌面端走系统保存对话框（`.txt`，原子写入）；网页版走浏览器下载，
 *   服务端不往用户机器上写任何路径（对齐 `canPersistErrorLog()` 的既有约定）。
 *
 * 演示模式：两个按钮都包在 {@link DemoAction} 里（禁用 + 解释），不会「点了才报错」。
 */
export function DiagnosticsCard() {
  const [report, setReport] = useState<DiagnosticsReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState("");

  /** 取一次报告（已有则复用，避免每次点都重扫盘）。 */
  async function load(force = false): Promise<DiagnosticsReport | null> {
    if (report && !force) return report;
    setLoading(true);
    setError("");
    try {
      const next = await api.getDiagnosticsReport();
      setReport(next);
      return next;
    } catch (e) {
      setError(api.asError(e));
      return null;
    } finally {
      setLoading(false);
    }
  }

  async function openPreview() {
    const loaded = await load();
    if (loaded) setPreviewOpen(true);
  }

  /** 复制全文：剪贴板失败必须如实提示，不能静默当成成功。 */
  async function copyAll() {
    if (!report) return;
    try {
      await navigator.clipboard.writeText(report.text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
      toast.success("诊断包已复制");
    } catch {
      toast.error("复制失败", {
        description: "浏览器未授予剪贴板权限，请手动选中预览内容后复制。",
      });
    }
  }

  async function exportReport() {
    const loaded = await load();
    if (!loaded) return;
    // 网页版：浏览器下载（不落服务端磁盘）。
    if (api.isWebui()) {
      downloadTextFile(fileNameFor(loaded), loaded.text);
      toast.success("已开始下载诊断包");
      return;
    }
    setExporting(true);
    try {
      const target = await api.saveFileDialog({
        defaultPath: fileNameFor(loaded),
        filters: [{ name: "文本", extensions: ["txt"] }],
      });
      if (!target) return;
      await api.exportDiagnostics(target);
      toast.success("诊断包已导出", { description: target });
    } catch (e) {
      toast.error("导出失败", { description: api.asError(e) });
    } finally {
      setExporting(false);
    }
  }

  const busy = loading || exporting;

  return (
    <SettingsGroup id="settings-diagnostics" title="诊断包">
      <CardContent className="space-y-0 p-0">
        <div className="border-b border-border/50 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
          反馈问题前先导出一份诊断包：含版本、宿主、客户端安装情况与最近错误日志，
          路径 / 令牌 / 邮箱等已自动脱敏，可直接粘贴到 issue。
        </div>
        <div className="flex flex-wrap gap-2 px-4 py-3 sm:px-5">
          <DemoAction>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void openPreview()}>
              {loading ? <Loader2 className="animate-spin" /> : null}
              预览诊断包
            </Button>
          </DemoAction>
          <DemoAction>
            <Button size="sm" disabled={busy} onClick={() => void exportReport()}>
              {exporting ? <Loader2 className="animate-spin" /> : <Download />}
              导出诊断包
            </Button>
          </DemoAction>
        </div>
        {report && (
          <div className="border-t border-border/50 px-4 py-2 text-[11px] leading-5 text-muted-foreground sm:px-5">
            上次生成：{new Date(report.generatedAt).toLocaleString()} · {sizeLabel(report.bytes)}
          </div>
        )}
        {error && (
          <div className="border-t border-border/50 px-4 py-2 text-[11px] leading-5 text-destructive sm:px-5">
            {error}
          </div>
        )}
      </CardContent>

      <Dialog open={previewOpen} onOpenChange={setPreviewOpen}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>诊断包预览</DialogTitle>
            <DialogDescription>
              以下文本就是将要导出的全部内容（已脱敏）。可滚动查看，确认后再复制或导出。
            </DialogDescription>
          </DialogHeader>
          <pre className="max-h-[60vh] overflow-auto rounded-lg border border-border/60 bg-foreground/[0.04] p-3 font-mono text-[11px] leading-5 whitespace-pre-wrap break-all">
            {report?.text ?? ""}
          </pre>
          <DialogFooter className="sm:justify-between">
            <Button variant="outline" size="sm" onClick={() => void load(true)} disabled={loading}>
              {loading ? <Loader2 className="animate-spin" /> : null}
              重新生成
            </Button>
            <Button size="sm" onClick={() => void copyAll()}>
              {copied ? <Check /> : <Copy />}
              {copied ? "已复制全文" : "复制全文"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </SettingsGroup>
  );
}
