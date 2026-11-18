import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";

import { AccountsHeader } from "./components/accounts-header";
import { AccountDialogs } from "./components/account-dialogs";
import { AccountsGrid } from "./components/accounts-grid";
import { AccountListToolbar } from "./components/account-list-toolbar";
import { CliSetupAlert } from "./components/cli-setup-alert";
import { AccountMigrationPanel } from "./components/migration-panel";
import { useAccountsPage } from "./hooks/use-accounts-page";

export default function AccountsPage() {
  const page = useAccountsPage();

  return (
    <div className="mx-auto w-full max-w-[1180px] px-6 py-8 sm:px-8 sm:py-9">
      <AccountsHeader
        variant={page.variant}
        onVariantChange={page.setVariant}
        status={page.status}
        clientStatus={page.clientStatus}
        enabledTools={page.enabledTools}
      />

      <AccountMigrationPanel
        variant={page.variant}
        accountCount={page.visibleAccounts.length}
        onAdd={() => page.setOauthOpen(true)}
        onImport={() => page.setImportOpen(true)}
        onExport={() => page.setExportOpen(true)}
      />

      {page.error && (
        <Alert variant="destructive" className="mb-4">
          <AlertTitle>加载失败</AlertTitle>
          <AlertDescription>{page.error}</AlertDescription>
        </Alert>
      )}

      <CliSetupAlert
        status={page.codebuddyCli}
        installing={page.installingCodebuddyCli}
        onInstall={() => void page.onInstallCodebuddyCli()}
      />

      <section className="mt-7 min-w-0" aria-labelledby="accounts-list-title">
        <AccountListToolbar
          accountCount={page.visibleAccounts.length}
          accountTypeLabel={page.accountTypeLabel}
          compact={page.compact}
          onToggleCompact={page.toggleCompact}
          busy={page.refreshingCredits || page.checkinAllRunning}
          refreshDisabled={page.visibleAccounts.length === 0}
          refreshLabel={page.refreshCreditsLabel}
          demoMode={page.demoMode}
          onRefresh={() => void page.onRefreshCredits()}
        />
        <AccountsGrid page={page} />
      </section>

      <AccountDialogs page={page} />
    </div>
  );
}
