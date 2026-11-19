import { ExternalLink } from "lucide-react";
import { toast } from "sonner";

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import * as api from "@/lib/api";
import { useAccountsStore } from "@/stores/accounts";

interface Props {
  error: string;
  needsPermission: boolean;
  permCheck: string | null;
  /** 打开系统「完全磁盘访问」设置面板（仅 macOS；失败由调用方提示）。 */
  onOpenPermissionSettings: () => void;
  /** 立即做一次权限自检。 */
  onRunPermissionCheck: () => void;
}

/**
 * 常驻错误提示：无权限时附带授权引导与一键操作。
 *
 * 与设置页「权限检测」卡片同一套平台差异（第三轮第 11 条）：macOS 是「完全磁盘访问」
 * 拖拽授权；Windows / Linux 没有这个开关，能做的只有定位认证文件与检查占用 / 只读。
 * 之前的实现把 macOS 文案和按钮无条件渲染出来，在 Windows 上点了毫无反应。
 */
export function SwitchAccountPermissionAlert({
  error,
  needsPermission,
  permCheck,
  onOpenPermissionSettings,
  onRunPermissionCheck,
}: Props) {
  const authFile = useAccountsStore((s) => s.status?.authFile);
  const desktop = api.isDesktop();
  const macos = api.desktopPlatform() === "macos";

  /** 定位认证文件：失败必须可见，不能 `void` 掉 reject。 */
  async function revealAuthFile() {
    try {
      if (!authFile) throw new Error("认证文件路径未获取");
      await api.revealPathInFileManager(authFile);
    } catch (e) {
      toast.error(api.asError(e));
    }
  }

  /** macOS 的 Finder 定位同样要暴露失败。 */
  async function revealApp() {
    try {
      await api.revealAppInFinder();
    } catch (e) {
      toast.error(api.asError(e));
    }
  }

  if (!error) return null;
  return (
    <Alert variant={needsPermission ? "warning" : "destructive"} className="min-w-0 shrink-0 break-all">
      <AlertDescription className="min-w-0 break-all">
        <div className="min-w-0 break-all">{error}</div>
        {needsPermission && (
          <div className="mt-2 space-y-2">
            {macos ? (
              <div className="rounded-md border bg-muted/60 p-3 text-xs text-muted-foreground">
                <p className="mb-1 font-medium text-foreground">如何授权（只需 3 步）：</p>
                <ol className="list-decimal space-y-1 pl-4">
                  <li>点击下方「打开完全磁盘访问」</li>
                  <li>
                    把 <b>WorkBuddy Shift.app</b> 从 Finder 拖进面板列表（即使没提示框也直接拖），
                    打开它的开关
                  </li>
                  <li>授权后这里会自动检测到，无需其他操作</li>
                </ol>
              </div>
            ) : (
              <div className="rounded-md border bg-muted/60 p-3 text-xs text-muted-foreground">
                <p className="mb-1 font-medium text-foreground">
                  写入认证文件失败的常见原因：
                </p>
                <ol className="list-decimal space-y-1 pl-4">
                  <li>WorkBuddy 客户端正在占用该文件：退出 WorkBuddy 后重试</li>
                  <li>文件只读或被 ACL 拒绝：检查文件「属性 → 安全」</li>
                  <li>杀毒软件或「受控文件夹访问」拦截写入：把 WorkBuddy Shift 加入白名单</li>
                </ol>
              </div>
            )}
            <div className="flex flex-wrap gap-2">
              {desktop && macos && (
                <>
                  <Button variant="outline" size="sm" onClick={onOpenPermissionSettings}>
                    <ExternalLink />
                    打开完全磁盘访问
                  </Button>
                  <Button variant="outline" size="sm" onClick={() => void revealApp()}>
                    在 Finder 中显示
                  </Button>
                </>
              )}
              {desktop && !macos && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={!authFile}
                  title={authFile ?? "认证文件路径未获取"}
                  onClick={() => void revealAuthFile()}
                >
                  {api.isWindows() ? "在资源管理器中显示认证文件" : "在文件管理器中显示认证文件"}
                </Button>
              )}
              <Button variant="secondary" size="sm" onClick={onRunPermissionCheck}>
                立即检测
              </Button>
            </div>
          </div>
        )}
        {permCheck && <div className="mt-2 text-xs">{permCheck}</div>}
      </AlertDescription>
    </Alert>
  );
}
