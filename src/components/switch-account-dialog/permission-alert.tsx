import { ExternalLink } from "lucide-react";

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import * as api from "@/lib/api";

interface Props {
  error: string;
  needsPermission: boolean;
  permCheck: string | null;
  /** 打开系统「完全磁盘访问」设置面板（失败由调用方提示）。 */
  onOpenPermissionSettings: () => void;
  /** 立即做一次权限自检。 */
  onRunPermissionCheck: () => void;
}

/** 常驻错误提示：无权限时附带「如何授权（只需 3 步）」引导与一键操作。 */
export function SwitchAccountPermissionAlert({
  error,
  needsPermission,
  permCheck,
  onOpenPermissionSettings,
  onRunPermissionCheck,
}: Props) {
  if (!error) return null;
  return (
    <Alert variant={needsPermission ? "warning" : "destructive"} className="min-w-0 shrink-0 break-all">
      <AlertDescription className="min-w-0 break-all">
        <div className="min-w-0 break-all">{error}</div>
        {needsPermission && (
          <div className="mt-2 space-y-2">
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
            <div className="flex flex-wrap gap-2">
              <Button variant="outline" size="sm" onClick={onOpenPermissionSettings}>
                <ExternalLink />
                打开完全磁盘访问
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void api.revealAppInFinder()}
              >
                在 Finder 中显示
              </Button>
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