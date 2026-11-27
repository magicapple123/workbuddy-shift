import { RotateCcw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";

/**
 * 去掉回包里的 `defaults` 字段。
 *
 * `defaults` 是后端随 `get_*_config` 一起下发的「出厂默认值」（纯增量字段），
 * 不是配置本体：整份回填时若把它一起提交，保存请求里就会多一个嵌套对象，
 * 落盘后的配置文件会多出这一层（下次导入设置包时还会被白名单当成未知字段拒绝）。
 */
export function stripDefaults<T extends object>(config: T): Omit<T, "defaults"> {
  const copy: Record<string, unknown> = { ...config } as Record<string, unknown>;
  delete copy.defaults;
  // 拷贝只是去掉了可选字段 `defaults`，其余键与类型完全一致，这里是一次形状已知的收窄。
  return copy as Omit<T, "defaults">;
}

interface RestoreDefaultsRowProps {
  /** 回填动作：由卡片把 `defaults` 提交给它自己的保存链。 */
  onRestore: () => void;
  /** 默认值还没拿到（配置未加载）或正在保存时禁用。 */
  disabled?: boolean;
  /** 说清「恢复默认」到底会改哪些东西（含不会改的），避免用户把它当重置按钮。 */
  description: string;
}

/**
 * 「恢复默认」行：设置页统一放在各配置卡的参数区末尾。
 *
 * 默认值**一律来自后端的 `defaults` 字段**，前端不硬编码任何数值：
 * 默认值写两份（core 一份、前端一份）必然漂移，而漂移的后果是用户点了「恢复默认」
 * 却得到一个既不是默认值、也不是自己设置值的中间状态。
 */
export function RestoreDefaultsRow({ onRestore, disabled, description }: RestoreDefaultsRowProps) {
  return (
    <div className="mt-1 flex min-w-0 items-center justify-between gap-3 border-t border-border/50 pt-2">
      <p className="min-w-0 text-xs leading-4 text-muted-foreground/75">{description}</p>
      <DemoAction>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="shrink-0 text-muted-foreground"
          disabled={disabled}
          onClick={onRestore}
        >
          <RotateCcw />
          恢复默认
        </Button>
      </DemoAction>
    </div>
  );
}
