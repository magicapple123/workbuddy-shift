import { useEffect, useState } from "react";
import { ExternalLink, FolderOpen, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import * as api from "@/lib/api";
import { useAccountsStore } from "@/stores/accounts";
import { GITHUB_REPOSITORY_URL, openReleaseUrl } from "@/lib/update";
import { SettingsFieldRow, SettingsGroup } from "./rows";

/** 问题反馈入口（GitHub Issues；与「自动更新」卡用的是同一个仓库）。 */
const ISSUES_URL = `${GITHUB_REPOSITORY_URL}/issues`;
/** 许可证原文（仓库根 `LICENSE`）。 */
const LICENSE_URL = `${GITHUB_REPOSITORY_URL}/blob/main/LICENSE`;

/**
 * 关于：版本、工具数据目录、许可证与反馈入口。
 *
 * 数据目录走新增的 `get_data_dir`（只回路径字符串，不含凭据），
 * 「打开」用 `storageReveal(..., scope = "cwd")` 而不是缺省的 `storage`：
 * 缺省 scope 只认 `~/.workbuddy-shift` 这类固定白名单根，而开发沙箱（`~/.wb-shift-dev`）
 * 与 `WB_SHIFT_HOME` 指向的目录不在白名单里，用 `storage` 会报「路径不在白名单根内」。
 * `cwd` scope 是只读定位（后端拒绝 UNC / 设备 / 相对路径，且绝不执行目标文件）。
 */
export function AboutCard() {
  const version = useAccountsStore((s) => s.status?.version);
  const [dataDir, setDataDir] = useState<string | null>(null);
  const [revealing, setRevealing] = useState(false);

  useEffect(() => {
    // 演示页不读本机路径：`get_data_dir` 不在演示白名单里，预取只会把错误提示先弹出来。
    if (api.isDemoMode()) return;
    let cancelled = false;
    void api
      .getDataDir()
      .then((info) => {
        if (!cancelled) setDataDir(info.path);
      })
      .catch(() => {
        // 读不到就显示占位文案：这里只是「关于」信息，失败不该变成错误弹窗。
        if (!cancelled) setDataDir("");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function openDataDir() {
    if (!dataDir) return;
    setRevealing(true);
    try {
      await api.storageReveal(dataDir, "open", "cwd");
    } catch (e) {
      toast.error("打开数据目录失败", { description: api.asError(e) });
    } finally {
      setRevealing(false);
    }
  }

  async function openExternal(url: string) {
    try {
      await openReleaseUrl(url);
    } catch (e) {
      toast.error("打开链接失败", { description: api.asError(e) });
    }
  }

  const dataDirText = api.isDemoMode()
    ? "演示模式不读取本机路径"
    : dataDir === null
      ? "正在读取…"
      : dataDir || "未能读取数据目录";

  return (
    <SettingsGroup id="settings-about" title="关于">
      <CardContent className="space-y-0 p-0">
        <SettingsFieldRow label="版本" description="当前安装的 WorkBuddy Shift 版本" htmlFor="about-version">
          <span id="about-version" className="font-mono text-sm">
            v{version || "?"}
          </span>
        </SettingsFieldRow>

        <SettingsFieldRow
          label="工具数据目录"
          description="账号库、配置、日志与备份都保存在这里；本机明文保存，请勿分享该目录"
        >
          <div className="flex min-w-0 w-full flex-col items-end gap-2 sm:w-auto sm:flex-row sm:items-center">
            <span className="max-w-full break-all text-right font-mono text-[11px] leading-4 text-muted-foreground sm:max-w-72">
              {dataDirText}
            </span>
            <DemoAction>
              <Button
                size="sm"
                variant="outline"
                className="shrink-0"
                disabled={revealing || !dataDir}
                onClick={() => void openDataDir()}
              >
                {revealing ? <Loader2 className="animate-spin" /> : <FolderOpen />}
                打开
              </Button>
            </DemoAction>
          </div>
        </SettingsFieldRow>

        <SettingsFieldRow label="许可证" description="MIT License，源代码公开在 GitHub">
          <DemoAction>
            <Button size="sm" variant="ghost" onClick={() => void openExternal(LICENSE_URL)}>
              MIT
              <ExternalLink />
            </Button>
          </DemoAction>
        </SettingsFieldRow>

        <SettingsFieldRow
          className="border-b-0"
          label="问题反馈"
          description="缺陷与需求请提到 GitHub Issues；附上版本号与错误日志（见上方「错误日志」）能更快定位"
        >
          <DemoAction>
            <Button size="sm" variant="ghost" onClick={() => void openExternal(ISSUES_URL)}>
              提交 Issue
              <ExternalLink />
            </Button>
          </DemoAction>
        </SettingsFieldRow>
      </CardContent>
    </SettingsGroup>
  );
}
