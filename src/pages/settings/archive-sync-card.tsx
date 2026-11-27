import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { setDefaultArchiveSyncEnabled, useDefaultArchiveSyncEnabled } from "@/lib/sync-prefs";
import { SettingsFieldRow, SettingsGroup } from "./rows";

/**
 * 「切号时同步」里唯一一项**本地偏好**（存 localStorage，不是后端配置）：
 * 切号弹窗是否预先勾选「只同步归档状态、保留正文」的关联会话。
 *
 * 从「外观」卡搬到这里（T05 第 4 项）：它是切号行为的一部分，与主题、导航显示
 * 没有关系；用户找「切号会不会动我的会话」时应该在「账号与同步」组里找。
 * 开关语义与落盘位置都没变，只换了展示位置。
 */
export function ArchiveSyncCard() {
  const defaultArchiveSyncEnabled = useDefaultArchiveSyncEnabled();

  return (
    <SettingsGroup id="settings-archive-sync" title="切号时同步归档">
      <CardContent className="space-y-0 p-0">
        <SettingsFieldRow
          className="border-b-0"
          label="切换账号时默认勾选归档同步"
          description="开启后，切换账号时会预先勾选可仅同步归档状态的关联会话，保留正文。你仍可取消勾选，确认切换后才执行。正文快进和覆盖沿用原有规则。"
          htmlFor="sync-default-archive"
        >
          <Switch
            id="sync-default-archive"
            checked={defaultArchiveSyncEnabled}
            onCheckedChange={(on) => setDefaultArchiveSyncEnabled(on)}
            aria-label="切换账号时默认勾选归档同步"
          />
        </SettingsFieldRow>
      </CardContent>
    </SettingsGroup>
  );
}
