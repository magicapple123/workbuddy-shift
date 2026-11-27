import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Check, Copy, ExternalLink, LogIn } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import { failureDiagnostic, type ApiFailure, type RecoveryAction } from "@/lib/api";
import { pagesDemoHostingEnabled } from "@/lib/demo-mode";
import { isSectionVisible } from "@/pages/settings/sections";
import { cn } from "@/lib/utils";

interface Props {
  /** 结构化失败；`null`（或没有动作）时整块不渲染。 */
  failure: ApiFailure | null;
  /** 来源标识，写进「复制诊断」的文本，便于作者定位是哪一处报的错。 */
  source: string;
  /**
   * `relogin` 的落点行为。**缺省时不渲染该按钮**：
   * 不同界面复用同一套动作，但「重新登录」在 OAuth 弹窗里是「重新发起扫码」，
   * 在别处没有对应入口时就不该留一个点了没用的按钮。
   */
  onRelogin?: () => void;
  className?: string;
}

/** `check_proxy` 的深链目标节（设置 → 自动更新，里面才有「更新代理地址」）。 */
const PROXY_SECTION_ID = "settings-updates";

/**
 * 失败动作行：按**档位映射出的动作**渲染「下一步做什么」。
 *
 * 三档动作各自复用既有能力，**不新造流程**：
 * - `relogin` → 调用方给的 `onRelogin`（OAuth 弹窗里就是它自己的「重新发起登录」）；
 * - `check_proxy` → 跳到设置页「自动更新」节（项目既有 hash 深链机制）；
 * - `copy_diagnostic` → 复制一段脱敏诊断摘要并 toast 反馈。
 *
 * 三条「不留死按钮」的约束：
 * 1. 没有动作时返回 `null`——**不留空按钮、不留空位、不留空 div**；
 * 2. `relogin` 没有落点时不渲染该按钮；
 * 3. `check_proxy` 的目标节在当前环境不可见（`settings-updates` 是
 *    `desktop-or-demo`，非演示 WebUI 没有这一节）时也不渲染——`SettingsToc` 找不到
 *    节点是静默 `return`，留着就是一个点了没反应的按钮。同理，GitHub Pages 部署用的是
 *    `HashRouter`（`App.tsx` 的 `pagesDemoHostingEnabled`），那里 `#` 已被路由占用、
 *    追加锚点没有可用形态，因此这种形态下同样不渲染。
 *
 * 演示模式下按钮包在 {@link DemoAction} 里，点击只解释「演示页不执行」，不会「点了才报错」。
 */
export function FailureActions({ failure, source, onRelogin, className }: Props) {
  const navigate = useNavigate();
  const [copied, setCopied] = useState(false);

  const action: RecoveryAction | null = failure?.action ?? null;
  const diagnostic = failure ? failureDiagnostic(failure, { source }) : "";

  if (action === null) return null;
  if (action === "relogin" && !onRelogin) return null;
  if (action === "check_proxy" && (pagesDemoHostingEnabled || !isSectionVisible(PROXY_SECTION_ID))) {
    return null;
  }

  /** 复制脱敏诊断摘要：失败必须如实提示，不能静默当成成功。 */
  async function copyDiagnostic() {
    try {
      await navigator.clipboard.writeText(diagnostic);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
      toast.success("诊断信息已复制", {
        description: "可直接粘贴到反馈里；需要更完整的信息可在「设置 → 诊断包」导出。",
      });
    } catch {
      toast.error("复制失败", {
        description: "浏览器未授予剪贴板权限，可在「设置 → 诊断包」预览后手动复制。",
      });
    }
  }

  return (
    <div className={cn("flex flex-wrap gap-2 pt-2", className)}>
      {action === "relogin" && onRelogin && (
        <DemoAction>
          <Button variant="outline" size="sm" onClick={onRelogin}>
            <LogIn />
            重新登录
          </Button>
        </DemoAction>
      )}
      {action === "check_proxy" && (
        <DemoAction>
          <Button
            variant="outline"
            size="sm"
            onClick={() => navigate(`/settings#${PROXY_SECTION_ID}`)}
          >
            <ExternalLink />
            检查更新代理
          </Button>
        </DemoAction>
      )}
      {action === "copy_diagnostic" && (
        <DemoAction>
          <Button variant="outline" size="sm" onClick={() => void copyDiagnostic()}>
            {copied ? <Check /> : <Copy />}
            {copied ? "已复制诊断信息" : "复制诊断信息"}
          </Button>
        </DemoAction>
      )}
    </div>
  );
}
