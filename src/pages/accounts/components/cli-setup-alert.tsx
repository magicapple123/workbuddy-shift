import { Loader2, Terminal } from "lucide-react";

import { DemoAction } from "@/components/demo-action";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type { CodeBuddyCliStatus } from "@/lib/types";

interface Props {
  status: CodeBuddyCliStatus | null;
  installing: boolean;
  onInstall: () => void;
}

/**
 * CodeBuddy CLI 接入/升级提示。
 *
 * 只在状态确实需要处理时出现：未接入、helper 版本过旧、需要迁移、同步待处理或进行中。
 * 同步进行中是正常中间态：给说明但不给按钮，避免用户重复触发写入。
 */
export function CliSetupAlert({ status, installing, onInstall }: Props) {
  if (!status) return null;
  const usesSettingsEnv = status.authMode === "settings-env";
  const needsAttention =
    !status.configured ||
    (!usesSettingsEnv && !status.helperSupportsAccountIds) ||
    status.migrationRequired ||
    status.syncPending ||
    status.syncInProgress;
  if (!needsAttention) return null;

  return (
    <Alert className="mb-4">
      <Terminal />
      <AlertTitle>CodeBuddy CLI 接入</AlertTitle>
      <AlertDescription>
        <p>
          {usesSettingsEnv
            ? status.environmentOverride
              ? "检测到进程环境变量 CODEBUDDY_AUTH_TOKEN。它会覆盖 settings.json；请先从 Windows 用户或系统环境变量中删除它，再重启本应用与 CodeBuddy CLI。"
              : status.syncPending
                ? "Windows CLI 认证配置与当前账号 Token 已脱节。点击更新认证后写入最新 Token；当前运行会话不会切换，请由 ACP 重新加载会话或重启 CLI 后生效。"
                : status.syncInProgress
                  ? "保活刷新已更新账号 Token，正在同步到 CodeBuddy CLI 认证配置。稍候会自动完成，无需操作。"
                  : status.migrationRequired
                    ? "检测到旧版 Windows helper 配置。接入后会改用 settings.json 的 env.CODEBUDDY_AUTH_TOKEN，不再执行 helper。"
                    : "Windows 使用 CodeBuddy settings.json 中的认证 Token。保活刷新只更新后续启动使用的 Token；切换账号会先关闭正在运行的 CodeBuddy CLI，重新打开 CLI 后即用新账号。"
            : status.migrationRequired
              ? "检测到旧版 helper，请先升级；升级前不会将 CLI 切换显示为已验证。"
              : status.configured
                ? "当前 helper 仍按旧索引读取账号；升级后将按账号 ID 独立切换，账号增删也不会错位。"
                : "WorkBuddy 账号与积分功能可正常使用；如需从这里切换 CodeBuddy CLI 账号，点击下方按钮一键接入。"}
        </p>
        {!status.syncInProgress && (
          <DemoAction>
            <Button
              className="mt-2"
              size="sm"
              variant="outline"
              onClick={onInstall}
              disabled={installing}
            >
              {installing && <Loader2 className="animate-spin" />}
              {usesSettingsEnv
                ? status.configured ? "更新 CLI 认证" : "接入 CLI"
                : status.configured || status.migrationRequired ? "升级 CLI helper" : "接入 CLI"}
            </Button>
          </DemoAction>
        )}
      </AlertDescription>
    </Alert>
  );
}
