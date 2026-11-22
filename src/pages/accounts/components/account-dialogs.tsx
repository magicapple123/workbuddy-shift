import { AccountInfoDialog } from "@/components/account-info-dialog";
import { AccountMarkDialog } from "@/components/account-mark-dialog";
import { CodebuddyIdeSwitchAccountDialog } from "@/components/codebuddy-ide-switch-account-dialog";
import { ExportAccountsDialog } from "@/components/export-accounts-dialog";
import { ImportAccountsDialog } from "@/components/import-accounts-dialog";
import { JetbrainsSwitchDialog } from "@/components/jetbrains-switch-dialog";
import { OAuthLoginDialog } from "@/components/oauth-login-dialog";
import { SwitchAccountDialog } from "@/components/switch-account-dialog";
import { VscodeSwitchAccountDialog } from "@/components/vscode-switch-account-dialog";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { displayName } from "@/lib/account-display";

import type { AccountsPageController } from "../hooks/use-accounts-page";

/**
 * 账号页的全部弹窗组合：新增/导入导出、切号（WorkBuddy / CodeBuddy IDE / VS Code / JetBrains）、
 * 账号信息、CLI 接入与删除确认。开关与动作统一来自账号页控制器。
 */
export function AccountDialogs({ page }: { page: AccountsPageController }) {
  return (
    <>
      <OAuthLoginDialog open={page.oauthOpen} onOpenChange={page.setOauthOpen} variant={page.variant} />
      <ExportAccountsDialog
        open={page.exportOpen}
        onOpenChange={page.setExportOpen}
        accounts={page.visibleAccounts}
        onExported={page.onExported}
      />
      <ImportAccountsDialog
        open={page.importOpen}
        onOpenChange={page.setImportOpen}
        onImported={page.onImported}
        variant={page.variant}
      />
      <SwitchAccountDialog
        open={page.switchAccount !== null}
        onOpenChange={(open) => {
          if (!open) page.setSwitchAccount(null);
        }}
        account={page.switchAccount}
        onDone={() => {
          void page.fetchAll();
          void page.refreshCodebuddyCliStatus();
          void page.refreshCodebuddyCnIdeStatus();
        }}
      />
      <AccountInfoDialog
        open={page.infoTarget !== null}
        onOpenChange={(open) => {
          if (!open) page.setInfoTarget(null);
        }}
        account={page.infoTarget}
        onSaved={() => {
          void page.fetchAll();
        }}
      />
      {/* 账号标记（R5-04）：卡片图标与「…」菜单都打开这一个弹窗；积分 / 限额用于智能提示。 */}
      <AccountMarkDialog
        open={page.markTarget !== null}
        onOpenChange={(open) => {
          if (!open) page.setMarkTarget(null);
        }}
        account={page.markTarget}
        credit={page.markTarget ? page.creditMap[page.markTarget.id] : undefined}
        rateLimits={page.markTarget ? page.rateLimitMap[page.markTarget.id] : undefined}
        onSaved={() => {
          void page.fetchAll();
        }}
      />
      <CodebuddyIdeSwitchAccountDialog
        open={page.codebuddyIdeSwitchAccount !== null}
        onOpenChange={(open) => {
          if (!open) page.setCodebuddyIdeSwitchAccount(null);
        }}
        account={page.codebuddyIdeSwitchAccount}
        variant={page.variant}
        ideStatus={page.codebuddyCnIde}
        onDone={() => {
          // 切号成功会写入「最近切换」时间戳（issue #184），必须重拉账号库徽标才会变。
          void page.fetchAll();
          void page.refreshCodebuddyCnIdeStatus();
        }}
      />
      <VscodeSwitchAccountDialog
        open={page.vscodeSwitchAccount !== null}
        onOpenChange={(open) => {
          if (!open) page.setVscodeSwitchAccount(null);
        }}
        account={page.vscodeSwitchAccount}
        vscodeExtStatus={page.vscodeExt}
        onDone={() => {
          void page.fetchAll();
          void page.refreshVscodeExtStatus();
        }}
      />
      <JetbrainsSwitchDialog
        open={page.jetbrainsSwitchTarget !== null}
        onOpenChange={(open) => {
          if (!open) page.setJetbrainsSwitchTarget(null);
        }}
        account={page.jetbrainsSwitchTarget}
        jetbrainsStatus={page.jetbrains}
        onDone={() => {
          void page.fetchAll();
          void page.refreshJetbrainsStatus();
        }}
      />

      {/* 接入/升级 CLI 认证确认（桌面 App 不支持 window.confirm） */}
      <Dialog open={page.installConfirmOpen} onOpenChange={page.setInstallConfirmOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {page.codebuddyUsesSettingsEnv
                ? "更新 CodeBuddy CLI 认证"
                : page.codebuddyCli?.configured || page.codebuddyCli?.migrationRequired
                  ? "升级 CodeBuddy CLI helper"
                  : "接入 CodeBuddy CLI"}
            </DialogTitle>
            <DialogDescription>
              {page.codebuddyUsesSettingsEnv ? (
                <>
                  将把当前账号的认证 Token 写入
                  <code className="mx-1 rounded bg-muted px-1">~/.codebuddy/settings.json</code>
                  的 <code className="mx-1 rounded bg-muted px-1">env.CODEBUDDY_AUTH_TOKEN</code>。
                  其他配置会保留；更新只影响后续加载的会话，当前运行会话不会切换。是否继续？
                </>
              ) : (
                <>
                  {page.codebuddyCli?.configured || page.codebuddyCli?.migrationRequired ? "升级" : "接入"}会自动写入
                  <code className="mx-1 rounded bg-muted px-1">~/.codebuddy-rotate/helper.cjs</code>
                  并更新
                  <code className="mx-1 rounded bg-muted px-1">~/.codebuddy/settings.json</code>
                  的 apiKeyHelper 配置，是否继续？
                </>
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => page.setInstallConfirmOpen(false)}>
              取消
            </Button>
            <Button onClick={() => void page.confirmInstallCodebuddyCli()}>继续</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 切换 CodeBuddy CLI 确认（桌面 App 不支持 window.confirm） */}
      <Dialog open={page.cliSwitchTarget !== null} onOpenChange={(open) => !open && page.setCliSwitchTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>切换 CodeBuddy CLI</DialogTitle>
            <DialogDescription>
              将把 CodeBuddy CLI 默认账号设为「{page.cliSwitchTarget ? displayName(page.cliSwitchTarget) : ""}」。
              确认后会关闭正在运行的 CodeBuddy CLI 会话，当前会话会中断；重新打开 CLI 后新账号才会生效。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => page.setCliSwitchTarget(null)}>
              取消
            </Button>
            <Button onClick={() => void page.confirmSwitchCodebuddyCli()}>
              关闭 CLI 并切换
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 删除账号确认 */}
      <Dialog open={page.deleteTarget !== null} onOpenChange={(open) => !open && page.setDeleteTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>删除账号</DialogTitle>
            <DialogDescription>
              确定删除账号「{page.deleteTarget ? displayName(page.deleteTarget) : ""}」？
              此操作不可撤销。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => page.setDeleteTarget(null)}>
              取消
            </Button>
            <Button variant="destructive" onClick={() => void page.confirmDelete()}>
              删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
