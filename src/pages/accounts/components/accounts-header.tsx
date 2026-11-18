import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { displayName } from "@/lib/account-display";
import type { ToolId } from "@/lib/supported-tools";
import type { AppStatus, WbVariant } from "@/lib/types";
import { normalizeVariant } from "@/lib/variant";
import type { ClientStatusSnapshot } from "@/stores/accounts";
import { ToolStatusMarks } from "../tool-status";

interface Props {
  variant: WbVariant;
  onVariantChange: (variant: WbVariant) => void;
  status: AppStatus | null;
  clientStatus: ClientStatusSnapshot;
  enabledTools: Record<ToolId, boolean>;
}

/** 账号页页头：标题、说明、档位切换与右侧工具状态标记。 */
export function AccountsHeader({ variant, onVariantChange, status, clientStatus, enabledTools }: Props) {
  const current = status?.current;
  const workbuddyCurrentName = current ? displayName(current) : "未登录";
  return (
    <header className="mb-6">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h1 className="text-[28px] font-semibold tracking-tight">账号管理</h1>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            统一管理 WorkBuddy、CodeBuddy IDE、CodeBuddy CLI 与 VS Code CodeBuddy 插件账号、积分和签到状态。
          </p>
          <Tabs
            className="mt-4 gap-0"
            value={variant}
            onValueChange={(value) => onVariantChange(normalizeVariant(value))}
          >
            <TabsList aria-label="WorkBuddy 档位">
              <TabsTrigger value="cn">国内版</TabsTrigger>
              <TabsTrigger value="ai">国际版</TabsTrigger>
            </TabsList>
          </Tabs>
        </div>
        <ToolStatusMarks
          variant={variant}
          enabledTools={enabledTools}
          status={status}
          clientStatus={clientStatus}
          workbuddyCurrentName={workbuddyCurrentName}
        />
      </div>
    </header>
  );
}
