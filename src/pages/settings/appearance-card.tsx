import { useState } from "react";
import { CardContent } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { setSessionsNavEnabled, useSessionsNavEnabled } from "@/lib/nav-prefs";
import { getThemePreference, setThemePreference, type ThemePreference } from "@/lib/theme";
import { SettingsFieldRow, SettingsGroup } from "./rows";

export function AppearanceCard() {
  const [theme, setTheme] = useState<ThemePreference>(getThemePreference);
  const sessionsNavEnabled = useSessionsNavEnabled();

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
          className="border-b-0"
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
      </CardContent>
    </SettingsGroup>
  );
}
