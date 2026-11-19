import { useState } from "react";
import { toast } from "sonner";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import * as api from "@/lib/api";
import { DemoAction } from "@/components/demo-action";
import { useAccountsStore } from "@/stores/accounts";
import { SettingsGroup } from "./rows";
import { useAuthFile } from "./update-card";

/**
 * 权限检测卡片：确认本 App 是否有权写入 WorkBuddy 认证文件（探针与展示路径同档位）。
 *
 * 平台差异（第三轮第 11 条）：
 * - macOS：「完全磁盘访问」「App 管理」是真实存在的系统开关，三个按钮保持原样；
 * - Windows / Linux：没有这两个授权项，诊断目标是**认证文件**能不能写，
 *   因此只留「检测权限」+「在资源管理器中显示认证文件」；
 * - WebUI：没有桌面文件管理器，三个桌面按钮整体不渲染，「检测权限」仍然给出
 *   服务进程权限说明。
 *
 * 所有按钮都必须 try/catch 并给可见反馈：后端已经不再吞 spawn 失败，前端也不能用
 * 裸 `void` 把 reject 丢掉（那正是「点了完全没反应」的另一半原因）。
 */
export function PermissionCheckCard() {
  const authFile = useAuthFile();
  const variant = useAccountsStore((s) => s.variant);
  const [checking, setChecking] = useState(false);
  /** 只在失败时留在卡片内：错误文案与授权引导不该被几秒的 toast 吞掉。 */
  const [error, setError] = useState<string | null>(null);
  /** 后端按平台给出的写入失败原因（macOS 是授权，Windows 是占用 / ACL）。 */
  const [hint, setHint] = useState<string | null>(null);

  const platform = api.desktopPlatform();
  const desktop = api.isDesktop();
  const macos = platform === "macos";

  /** 统一的「点一下 → 一定看到结果」包装：成功给 toast，失败给 toast.error。 */
  async function run(action: () => Promise<void>) {
    try {
      await action();
    } catch (e) {
      toast.error(api.asError(e));
    }
  }

  async function runCheck() {
    setChecking(true);
    setError(null);
    setHint(null);
    try {
      const res = await api.checkAuthPermission(variant);
      if (res.ok) {
        toast.success(res.message ?? "认证目录可写，权限正常");
      } else {
        setError(`${res.error ?? "写入认证文件失败"}${res.dir ? `（${res.dir}）` : ""}`);
        setHint(res.hint ?? null);
      }
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setChecking(false);
    }
  }

  return (
    <SettingsGroup
      id="settings-permission"
      title="权限检测"
    >
      <CardContent className="space-y-0 p-0">
        <div className="break-all border-b border-border/60 bg-muted/25 px-4 py-3 font-mono text-[11px] leading-5 text-muted-foreground sm:px-5">
          {authFile || "认证文件路径未获取"}
        </div>
        <div className="flex flex-wrap gap-2 border-b-0 border-border/60 px-4 py-3 sm:px-5">
          <DemoAction><Button size="sm" onClick={runCheck} disabled={checking} aria-busy={checking}>
            {checking ? "检测中…" : "检测权限"}
          </Button></DemoAction>
          {desktop && macos && (
            <>
              <DemoAction><Button
                size="sm"
                variant="outline"
                onClick={() => void run(() => api.openPermissionSettings("all_files"))}
              >
                打开完全磁盘访问
              </Button></DemoAction>
              <DemoAction><Button
                size="sm"
                variant="outline"
                onClick={() => void run(() => api.openPermissionSettings("app_management"))}
              >
                打开 App 管理
              </Button></DemoAction>
              <DemoAction><Button
                size="sm"
                variant="outline"
                onClick={() => void run(() => api.revealAppInFinder())}
              >
                在 Finder 中显示
              </Button></DemoAction>
            </>
          )}
          {desktop && !macos && (
            <DemoAction><Button
              size="sm"
              variant="outline"
              disabled={!authFile}
              title={authFile ?? "认证文件路径未获取"}
              onClick={() =>
                void run(() =>
                  authFile
                    ? api.revealPathInFileManager(authFile)
                    : Promise.reject(new Error("认证文件路径未获取")),
                )}
            >
              {platform === "windows" ? "在资源管理器中显示认证文件" : "在文件管理器中显示认证文件"}
            </Button></DemoAction>
          )}
        </div>

        {error && (
          <Alert variant="destructive" className="!w-auto mx-4 my-4 sm:mx-5">
            <AlertDescription>
              {error}
              {hint && <span className="mt-1 block text-xs">{hint}</span>}
            </AlertDescription>
          </Alert>
        )}
        {error && macos && (
          <div className="mx-4 mb-4 border-l-2 border-destructive/50 bg-muted/30 px-3 py-2.5 text-xs text-muted-foreground sm:mx-5">
            <p className="mb-1 font-medium text-foreground">如何授权（拖拽方式）：</p>
            <ol className="list-decimal space-y-1 pl-4">
              <li>点上方「打开完全磁盘访问」</li>
              <li>再点「在 Finder 中显示」打开 WorkBuddy Shift 所在位置</li>
              <li>
                把 <b>WorkBuddy Shift.app</b> 从 Finder <b>直接拖进</b>完全磁盘访问的列表区域
                （即使没有提示框，拖入即生效），然后打开它的开关
              </li>
              <li>回到本页点「检测权限」，或直接重试切换</li>
            </ol>
          </div>
        )}
        {error && !macos && desktop && (
          <div className="mx-4 mb-4 border-l-2 border-destructive/50 bg-muted/30 px-3 py-2.5 text-xs text-muted-foreground sm:mx-5">
            <p className="mb-1 font-medium text-foreground">
              写入认证文件失败的常见原因（按出现频率排序）：
            </p>
            <ol className="list-decimal space-y-1 pl-4">
              <li>WorkBuddy 客户端正在运行并占用该文件：先退出 WorkBuddy 再重试</li>
              <li>
                文件只读或被 ACL 拒绝：右键文件 →「属性」→「安全」，确认当前用户有「写入」
                权限，并取消「只读」
              </li>
              <li>
                杀毒软件或 Windows「受控文件夹访问」拦截了写入：把 WorkBuddy Shift
                加入白名单后再试
              </li>
              <li>点上方「在资源管理器中显示认证文件」，确认该路径确实存在且是普通文件</li>
            </ol>
          </div>
        )}
      </CardContent>
    </SettingsGroup>
  );
}
