import { useState } from "react";
import { CardContent } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { setSessionsNavEnabled, useSessionsNavEnabled } from "@/lib/nav-prefs";
import { setDefaultArchiveSyncEnabled, useDefaultArchiveSyncEnabled } from "@/lib/sync-prefs";
import { getThemePreference, setThemePreference, type ThemePreference } from "@/lib/theme";
import { SettingsFieldRow, SettingsGroup } from "./rows";

export function AppearanceCard() {
  const [theme, setTheme] = useState<ThemePreference>(getThemePreference);
  const sessionsNavEnabled = useSessionsNavEnabled();
  const defaultArchiveSyncEnabled = useDefaultArchiveSyncEnabled();

  function onThemeChange(value: string) {
    if (value !== "system" && value !== "light" && value !== "dark") return;
    setThemePreference(value);
    setTheme(value);
  }

  return (
    <SettingsGroup
      id="settings-appearance"
      title="外观"
    >
      <CardContent className="space-y-0 p-0">
        <SettingsFieldRow
          label="主题"
          description="选择浅色、深色，或跟随系统外观自动切换"
          htmlFor="appearance-theme"
        >
          <Select value={theme} onValueChange={onThemeChange}>
            <SelectTrigger id="appearance-theme" size="sm" className="w-full sm:w-40" aria-label="主题">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">系统</SelectItem>
              <SelectItem value="light">浅色</SelectItem>
              <SelectItem value="dark">深色</SelectItem>
            </SelectContent>
          </Select>
        </SettingsFieldRow>
        <SettingsFieldRow
          label="显示关联会话菜单"
          description="关闭后左侧导航不再显示「关联会话」入口，会话数据与关联关系不受影响"
          htmlFor="appearance-sessions-nav"
        >
          <Switch
            id="appearance-sessions-nav"
            checked={sessionsNavEnabled}
            onCheckedChange={(on) => setSessionsNavEnabled(on)}
            aria-label="显示关联会话菜单"
          />
        </SettingsFieldRow>
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
