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

/** 权限检测卡片：确认本 App 是否有权写入 WorkBuddy 认证文件（探针与展示路径同档位）。 */
export function PermissionCheckCard() {
  const authFile = useAuthFile();
  const variant = useAccountsStore((s) => s.variant);
  const [checking, setChecking] = useState(false);
  /** 只在失败时留在卡片内：错误文案与授权四步引导不该被几秒的 toast 吞掉。 */
  const [error, setError] = useState<string | null>(null);

  async function runCheck() {
    setChecking(true);
    setError(null);
    try {
      const res = await api.checkAuthPermission(variant);
      if (res.ok) {
        toast.success(res.message ?? "认证目录可写，权限正常");
      } else {
        setError(`${res.error}（${res.dir ?? ""}）`);
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
          <DemoAction><Button size="sm" onClick={runCheck} disabled={checking}>
            {checking ? "检测中…" : "检测权限"}
          </Button></DemoAction>
          <DemoAction><Button
            size="sm"
            variant="outline"
            onClick={() => void api.openPermissionSettings("all_files")}
          >
            打开完全磁盘访问
          </Button></DemoAction>
          <DemoAction><Button
            size="sm"
            variant="outline"
            onClick={() => void api.openPermissionSettings("app_management")}
          >
            打开 App 管理
          </Button></DemoAction>
          <DemoAction><Button size="sm" variant="outline" onClick={() => void api.revealAppInFinder()}>
            在 Finder 中显示
          </Button></DemoAction>
        </div>

        {error && (
          <Alert variant="destructive" className="!w-auto mx-4 my-4 sm:mx-5">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        {error && (
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
      </CardContent>
    </SettingsGroup>
  );
}
