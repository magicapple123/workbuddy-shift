

import * as api from "@/lib/api";
import { AppearanceCard } from "./appearance-card";
import { AutoCheckinCard } from "./auto-checkin-card";
import { AutoRotateCard } from "./auto-rotate-card";
import { CompanionCard } from "./companion-card";
import { ErrorLogCard } from "./error-log-card";
import { NotificationHistoryCard } from "./notification-history-card";
import { PermissionCheckCard } from "./permission-check-card";
import { RateLimitCard } from "./rate-limit-card";
import { StartupCard } from "./startup-card";
import { SupportedToolsCard } from "./supported-tools-card";
import { AutomationSyncCard } from "./automation-sync-card";
import { SettingsSyncCard } from "./settings-sync-card";
import { UpdateCard } from "./update-card";

/** 设置页：演示模式不渲染自动签到；会话悬浮窗只在桌面正式版显示。 */
export default function SettingsPage() {
  return (
    <div className="mx-auto min-w-0 w-full max-w-3xl px-4 py-6 sm:px-6 sm:py-8">
      <header className="mb-10 sm:mb-12">
        <h1 className="text-2xl font-semibold tracking-tight">设置</h1>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">
          {api.isDemoMode() ? "限额监听、权限检测与自动更新配置。" : "自动签到、限额监听、权限检测与自动更新配置。"}
        </p>
      </header>

      <div className="min-w-0 space-y-12">
        <AppearanceCard />
        <SupportedToolsCard />
        {api.isDemoMode() ? null : <SettingsSyncCard />}
        {api.isDemoMode() ? null : <AutomationSyncCard />}
        <PermissionCheckCard />
        {api.isDemoMode() ? null : <AutoCheckinCard />}
        <AutoRotateCard />
        <RateLimitCard />
        {api.isDesktop() && !api.isDemoMode() ? <CompanionCard /> : null}
        {api.isDesktop() || api.isDemoMode() ? <StartupCard /> : null}
        <NotificationHistoryCard />
        <ErrorLogCard />
        {api.isWebui() && !api.isDemoMode() ? null : <UpdateCard />}
      </div>
    </div>
  );
}
