import { lazy, Suspense, useEffect, useState } from "react";
import { BrowserRouter, HashRouter, Navigate, NavLink, Outlet, Route, Routes } from "react-router-dom";
import { Database, Link2, Loader2, MessagesSquare, Settings, Sparkles, User } from "lucide-react";

import { cn } from "@/lib/utils";
import * as api from "@/lib/api";
// 路由级分包：统计页带图表库（recharts 约 380 KB），不该跟着首屏加载。
const AccountsPage = lazy(() => import("@/pages/accounts"));
const SessionsPage = lazy(() => import("@/pages/sessions"));
const CreditStatsPage = lazy(() => import("@/pages/credit-stats"));
const TokenStatsPage = lazy(() => import("@/pages/token-stats"));
const SettingsPage = lazy(() => import("@/pages/settings"));
const StoragePage = lazy(() => import("@/pages/storage"));
import { AppIconMark } from "@/components/product-marks";
import { Badge } from "@/components/ui/badge";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { UpdateCenter } from "@/components/app-shell/update-center";
import { demoModeEnabled, pagesDemoHostingEnabled } from "@/lib/demo-mode";
import { useSessionsNavEnabled } from "@/lib/nav-prefs";
import { useCreditAutoRefresh } from "@/hooks/use-credit-auto-refresh";
import { useRotateDeferredNotice } from "@/hooks/use-rotate-deferred-notice";
import { useStatsWarmup } from "@/hooks/use-stats-warmup";
import { useStorageWarmup } from "@/hooks/use-storage-warmup";
import { useWorkbuddyStatusRefresh } from "@/hooks/use-workbuddy-status-refresh";
import { useAccountsStore } from "@/stores/accounts";

/**
 * 侧栏导航项的选中态：主色淡底 + 主色文字，图标单独走 `--primary`。
 * 6 个 NavLink 共用一份定义，避免「只改第一个」的漂移；不要再用 `bg-foreground/[0.06]` 灰块。
 */
function sidebarNavItemClass(isActive: boolean): string {
  return cn(
    "flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-sidebar-ring/50",
    isActive
      ? "bg-primary-soft font-medium text-primary-soft-foreground [&>svg]:text-primary"
      : "text-muted-foreground hover:bg-primary-soft/60 hover:text-foreground",
  );
}

function Layout() {
  const running = useAccountsStore((s) => s.status?.running);
  const hasUnifiedTitleBar =
    api.isDesktop() && typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");
  const sessionsNavEnabled = useSessionsNavEnabled();
  /** 沙箱信息：`null` = 加载中（不渲染角标，避免先闪出再消失）。 */
  const [devEnv, setDevEnv] = useState<{ sandboxed: boolean; home: string; realHome: string } | null>(
    null,
  );
  useCreditAutoRefresh();
  useWorkbuddyStatusRefresh();
  useRotateDeferredNotice();
  // 启动预热：空闲时先把账号 / 积分 / Token 统计备好，进统计页直接渲染（一次会话只跑一次）。
  useStatsWarmup();
  // 存储总览预热排在统计之后（统计优先），只预热总览；会话资产由存储页按需加载。
  useStorageWarmup();

  // 沙箱角标只查一次：环境变量在进程生命周期内不变，无需轮询。
  useEffect(() => {
    let cancelled = false;
    api
      .getDevEnvironment()
      .then((info) => {
        if (!cancelled) setDevEnv(info);
      })
      .catch(() => {
        // 兜底同 api 层的降级：读不到就按非沙箱渲染，不影响主界面。
        if (!cancelled) setDevEnv({ sandboxed: false, home: "", realHome: "" });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="flex h-screen min-h-0 overflow-hidden bg-background">
      {hasUnifiedTitleBar ? (
        <div
          data-tauri-drag-region
          className="fixed inset-x-0 top-0 z-50 h-8"
          aria-hidden="true"
        />
      ) : null}
      <aside
        className={cn(
          "flex min-h-0 w-[220px] shrink-0 flex-col border-r border-sidebar-border bg-sidebar px-3 pb-4",
          hasUnifiedTitleBar ? "pt-20" : "pt-4",
        )}
      >
        <div className="flex items-center gap-2.5 px-1 pb-5">
          <AppIconMark size={36} className="drop-shadow-sm" />
          <div className="min-w-0">
            <div className="flex items-center gap-1.5">
              <div
                className="truncate text-[15px] leading-5 tracking-[-0.02em] text-sidebar-foreground/90"
                style={{
                  fontFamily: '"Bricolage Grotesque Variable", "SF Pro Display", ui-sans-serif, sans-serif',
                  fontWeight: 640,
                }}
              >
                WorkBuddy Shift
              </div>
              {/* dev 构建被 WB_SHIFT_HOME 重定向时标记沙箱：数据全部来自沙箱副本，
                  必须和正式版一眼可分。警告赭黄刻意避开「演示模式」的绿色。 */}
              {devEnv?.sandboxed && (
                <Badge
                  variant="secondary"
                  title={`开发沙箱：${devEnv.home}（真实数据：${devEnv.realHome}）`}
                  className="h-5 shrink-0 border border-warning-border bg-warning-soft px-1.5 text-[10px] text-warning shadow-none"
                >
                  沙箱
                </Badge>
              )}
            </div>
            {demoModeEnabled && (
              <Badge variant="secondary" className="mt-1 h-5 border-0 px-1.5 text-[10px] text-sidebar-foreground/60 shadow-none">
                演示模式
              </Badge>
            )}
          </div>
        </div>
        <nav className="flex min-h-0 flex-1 flex-col gap-0.5" aria-label="主导航">
          <NavLink
            to="/"
            end
            className={({ isActive }) => sidebarNavItemClass(isActive)}
          >
            <User className="size-4" />
            账号管理
          </NavLink>
          {sessionsNavEnabled && (
            <NavLink
              to="/sessions"
              className={({ isActive }) =>
                cn(
                  "flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-sidebar-ring/50",
                  isActive
                    ? "bg-foreground/[0.06] font-medium text-foreground"
                    : "text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
                )
              }
            >
              <Link2 className="size-4" />
              关联会话
            </NavLink>
          )}
          <NavLink to="/token-stats" className={({ isActive }) => sidebarNavItemClass(isActive)}><MessagesSquare className="size-4" />Token 统计</NavLink>
          <NavLink
            to="/credit-stats"
            className={({ isActive }) => sidebarNavItemClass(isActive)}
          >
            <Sparkles className="size-4" />
            积分统计
          </NavLink>
          <NavLink
            to="/storage"
            className={({ isActive }) => sidebarNavItemClass(isActive)}
          >
            <Database className="size-4" />
            存储管理
          </NavLink>
          <NavLink
            to="/settings"
            className={({ isActive }) => sidebarNavItemClass(isActive)}
          >
            <Settings className="size-4" />
            设置
          </NavLink>
        </nav>
        {api.isWebui() && !demoModeEnabled ? null : <UpdateCenter running={running} />}
      </aside>
      <main
        className={cn(
          "min-w-0 flex-1 overflow-y-auto bg-background overscroll-contain",
          hasUnifiedTitleBar && "pt-16 [&>div]:pt-4",
        )}
      >
        <Outlet />
      </main>
    </div>
  );
}

/** 路由分包后的占位：只占内容区，侧边栏与顶栏保持不动。 */
function RouteFallback() {
  return (
    <div className="flex h-64 items-center justify-center text-muted-foreground" role="status" aria-live="polite">
      <Loader2 className="size-5 animate-spin" />
      <span className="sr-only">加载中</span>
    </div>
  );
}

export default function App() {
  const Router = pagesDemoHostingEnabled ? HashRouter : BrowserRouter;

  return (
    <TooltipProvider delayDuration={250}>
      <Router>
        <Suspense fallback={<RouteFallback />}>
          <Routes>
          <Route element={<Layout />}>
            <Route path="/" element={<AccountsPage />} />
            <Route path="/sessions" element={<SessionsPage />} />
            <Route path="/credit-stats" element={<CreditStatsPage />} />
            <Route path="/token-stats" element={<TokenStatsPage />} />
            <Route path="/storage" element={<StoragePage />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Route>
          </Routes>
        </Suspense>
        <Toaster />
      </Router>
    </TooltipProvider>
  );
}
