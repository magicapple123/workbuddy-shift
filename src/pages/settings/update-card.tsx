import { useEffect, useState } from "react";
import { ArrowUpCircle, ExternalLink, Loader2, RefreshCw, Save } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import type {
  GithubConfig,
  UpdateInfo,
} from "@/lib/types";
import { GITHUB_RELEASE_URL, GITHUB_REPOSITORY_URL, openReleaseUrl } from "@/lib/update";
import { useUpdateState } from "@/hooks/use-update-state";
import { demoModeEnabled } from "@/lib/demo-mode";
import { cn } from "@/lib/utils";
import { openUpdateDialog } from "@/components/update-host";
import { publishedLabel } from "@/components/update-prompt";
import { DemoAction } from "@/components/demo-action";
import { useAccountsStore } from "@/stores/accounts";
import { isProxyDirty, normalizeProxy, validateProxyUrl } from "./helpers";
import { SettingsFieldRow, SettingsGroup } from "./rows";

export function useAuthFile(): string | undefined {
  return useAccountsStore((s) => s.status?.authFile);
}

/**
 * 每秒刷新的「已持续秒数」：后台检查 / 下载没有进度条，至少让用户看到在动。
 * `since` 为 null 时停止计时。
 */
function useElapsedSeconds(since: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [since]);
  if (since === null) return 0;
  return Math.max(0, Math.floor((now - since) / 1000));
}

/** 发布标题与版本号重复时不显示（第 3 条：`v0.2.0 · v0.2.0`）。 */
function usableReleaseName(info: UpdateInfo | null): string | undefined {
  const name = info?.releaseName?.trim();
  if (!name || !info) return undefined;
  const duplicates = [info.latest, info.current].flatMap((version) =>
    version ? [version, `v${version}`] : [],
  );
  return duplicates.includes(name) ? undefined : name;
}

/** 自动更新：检查公开 GitHub Releases 源 + 安装签名更新。 */
export function UpdateCard() {
  const version = useAccountsStore((s) => s.status?.version);
  // 阶段与进度来自 Rust 更新服务（托盘同源）：下载完成时按钮换成「重启以完成升级」。
  const snapshot = useUpdateState();
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [githubConfig, setGithubConfig] = useState<GithubConfig>({});
  /** 配置是否已加载：加载前保存会把 owner/repo/proxy 一起写成默认值，必须禁掉开关。 */
  const [configLoaded, setConfigLoaded] = useState(false);
  const [proxyUrl, setProxyUrl] = useState("");
  const [proxySaving, setProxySaving] = useState(false);
  const [autoCheckSaving, setAutoCheckSaving] = useState(false);
  // 后台检查没有本地起点：观察到 checking 的那一刻开始计时。
  const [checkingSince, setCheckingSince] = useState<number | null>(null);
  const [busySince, setBusySince] = useState<number | null>(null);

  const backgroundChecking = snapshot.phase === "checking";
  const downloading = snapshot.phase === "downloading";
  const checkBusy = checking || backgroundChecking || downloading;
  const checkingElapsed = useElapsedSeconds(checkingSince);
  const busyElapsed = useElapsedSeconds(busySince);
  /** 输入框相对「已保存值」是否有实质改动（忽略首尾空白）。 */
  const proxyDirty = isProxyDirty(proxyUrl, githubConfig.proxy);
  /** 清空已保存的代理：按钮文案改成「关闭代理」。 */
  const clearingProxy = !normalizeProxy(proxyUrl) && Boolean(normalizeProxy(githubConfig.proxy));
  const autoCheckEnabled = githubConfig.autoCheck !== false;

  useEffect(() => {
    if (snapshot.phase === "checking") {
      setCheckingSince((current) => current ?? Date.now());
    } else {
      setCheckingSince(null);
    }
    if (snapshot.busyKind && snapshot.busySince) {
      setBusySince(snapshot.busySince * 1000);
    } else if (!snapshot.busyKind) {
      setBusySince(null);
    }
  }, [snapshot.phase, snapshot.busyKind, snapshot.busySince]);

  useEffect(() => {
    let cancelled = false;
    void api
      .getGithubConfig()
      .then((config) => {
        if (cancelled) return;
        setGithubConfig(config);
        setProxyUrl(config.proxy ?? "");
        setConfigLoaded(true);
      })
      .catch((e) => {
        // 加载失败保持开关禁用：此时保存会用空对象覆盖 owner/repo/proxy。
        if (!cancelled) toast.error("更新配置加载失败", { description: api.asError(e) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  /**
   * 保存代理地址。
   *
   * - 返回是否成功：调用方（检查更新）失败时必须中止，不能拿旧配置继续。
   * - `silent=true` 用于检查前的自动保存：用户没点按钮，成功不弹提示（按钮会自动消失），
   *   但失败仍要提示——否则检查会静默地走旧代理，用户只看到「检查失败」。
   * - 演示模式不真的落盘，自动保存失败时也不弹真实错误。
   */
  async function saveProxy(silent = false): Promise<boolean> {
    const value = normalizeProxy(proxyUrl);
    const issue = validateProxyUrl(value);
    if (issue) {
      if (!silent || !demoModeEnabled) toast.error(issue);
      return false;
    }

    setProxySaving(true);
    try {
      const saved = await api.saveGithubConfig({ ...githubConfig, proxy: value });
      setGithubConfig(saved);
      // 用后端回写的值：保存成功后按钮行随之消失（`proxyDirty` 归 false）。
      setProxyUrl(saved.proxy ?? "");
      if (!silent) toast.success(value ? "更新代理已保存" : "已关闭更新代理");
      return true;
    } catch (e) {
      if (!(silent && demoModeEnabled)) {
        toast.error("保存代理失败", { description: api.asError(e) });
      }
      return false;
    } finally {
      setProxySaving(false);
    }
  }

  /** 检查更新：先落盘脏代理（格式非法则中止），再走后端检查。 */
  async function check() {
    setChecking(true);
    try {
      if (proxyDirty) {
        const issue = validateProxyUrl(proxyUrl);
        if (issue) {
          toast.error("代理地址格式不正确", { description: issue });
          return;
        }
        // 静默保存：成功后按钮消失，失败已由 saveProxy 提示，这里只中止检查。
        const saved = await saveProxy(true);
        if (!saved) return;
      }
      const r = await api.checkUpdate(normalizeProxy(proxyUrl), true);
      setInfo(r);
      if (!r.ok) {
        // BUSY 是「有任务在跑」而不是失败：用 info 级别的提示，不吓用户。
        const busy = /正在进行|请稍候/.test(r.message ?? r.error ?? "");
        const notify = busy ? toast.info : toast.error;
        notify("检查更新失败", { description: r.message || r.error });
      }
    } catch (e) {
      toast.error("检查更新失败", { description: api.asError(e) });
    } finally {
      setChecking(false);
    }
  }

  /** 「启动时检查更新」开关：落盘后 Rust 后台轮询下一轮即生效（无需重启）。 */
  async function toggleAutoCheck(next: boolean) {
    setAutoCheckSaving(true);
    try {
      const saved = await api.saveGithubConfig({ ...githubConfig, autoCheck: next });
      setGithubConfig(saved);
      toast.success(
        next ? "已开启「启动时检查更新」" : "已关闭「启动时检查更新」，仅在手动点击时联网",
      );
    } catch (e) {
      toast.error("保存设置失败", { description: api.asError(e) });
    } finally {
      setAutoCheckSaving(false);
    }
  }

  /** 安装已下载的更新包并重启（下载完成后的主动作，与托盘菜单同一入口）。 */
  async function restartNow() {
    setRestarting(true);
    try {
      await api.updateRestart();
    } catch (e) {
      setRestarting(false);
      toast.error("重启失败", { description: api.asError(e) });
    }
  }

  return (
    <SettingsGroup
      id="settings-updates"
      title="自动更新"
    >
      <CardContent className="space-y-0 p-0">
        <div className="border-b border-border/60 px-4 py-3 text-sm sm:px-5">
          当前版本：<span className="font-mono">v{version || "?"}</span>
        </div>

        <div className="flex min-w-0 items-center justify-between gap-3 border-b border-border/60 bg-muted/25 px-4 py-3 text-sm sm:px-5">
          <div className="min-w-0 flex-1">
            <div className="font-medium">公开更新源</div>
            <div className="truncate text-xs text-muted-foreground">{GITHUB_REPOSITORY_URL}</div>
          </div>
          <DemoAction><Button
            variant="ghost"
            size="icon"
            title="打开 GitHub Release"
            onClick={() => void openReleaseUrl(GITHUB_RELEASE_URL)}
          >
            <ExternalLink />
          </Button></DemoAction>
        </div>

        <SettingsFieldRow
          label="启动时检查更新"
          description="关闭后仅在手动点击「检查更新」时联网，后台不再定时查询。"
          htmlFor="update-auto-check"
          className="bg-muted/25"
          operational
        >
          <Switch
            id="update-auto-check"
            checked={autoCheckEnabled}
            disabled={!configLoaded || autoCheckSaving}
            onCheckedChange={(next) => void toggleAutoCheck(next)}
            aria-label="启动时检查更新"
          />
        </SettingsFieldRow>

        <SettingsFieldRow
          label="更新代理地址"
          description="仅用于 GitHub 更新检查和安装包下载；留空表示关闭显式代理。"
          htmlFor="update-proxy"
          className="bg-muted/25"
          operational
        >
          <Input
            id="update-proxy"
            className="w-full sm:w-80"
            value={proxyUrl}
            onChange={(event) => setProxyUrl(event.target.value)}
            onKeyDown={(event) => {
              // 回车直接保存：代理只有一个字段，不必再去点按钮。
              if (event.key !== "Enter") return;
              event.preventDefault();
              void saveProxy();
            }}
            placeholder="例如 http://127.0.0.1:7897"
            spellCheck={false}
            autoComplete="off"
          />
        </SettingsFieldRow>

        {(proxyDirty || proxySaving) && (
          <div className="flex flex-wrap gap-2 border-b border-border/60 bg-muted/25 px-4 py-3 sm:px-5">
            <DemoAction><Button
              size="sm"
              variant="outline"
              onClick={() => void saveProxy()}
              disabled={proxySaving}
            >
              {proxySaving ? <Loader2 className="animate-spin" /> : <Save />}
              {clearingProxy ? "关闭代理" : "保存代理"}
            </Button></DemoAction>
          </div>
        )}

        <div className="flex flex-wrap items-center gap-2 border-b-0 border-border/60 px-4 py-3 sm:px-5">
          <DemoAction><Button size="sm" variant="outline" onClick={check} disabled={checkBusy}>
            {checkBusy ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            {downloading
              ? `正在下载更新（已 ${busyElapsed}s）`
              : backgroundChecking || checking
                ? `正在检查更新（已 ${checkingElapsed}s）`
                : "检查更新"}
          </Button></DemoAction>
          {backgroundChecking && (
            <span className="text-xs text-muted-foreground">后台正在检查，无需重复点击</span>
          )}
        </div>

        {info?.ok && (
          <Alert variant="default" className={cn("!w-auto mx-4 my-4 sm:mx-5", info.hasUpdate && "border-primary/35 bg-primary/[0.06]")}>
            {info.hasUpdate && <ArrowUpCircle className="text-primary" />}
            <AlertDescription className="space-y-2">
              <AlertTitle className={cn(info.hasUpdate && "text-primary")}>{info.hasUpdate ? "发现新版本" : "更新检查完成"}</AlertTitle>
              <div className="text-sm">
                {info.hasUpdate
                  ? `发现新版本 v${info.latest}（当前 v${info.current}）`
                  : `已是最新版本 v${info.current}`}
                {usableReleaseName(info) && (
                  <span className="text-muted-foreground"> · {usableReleaseName(info)}</span>
                )}
                {publishedLabel(info.publishedAt) && (
                  <span className="text-muted-foreground"> · {publishedLabel(info.publishedAt)}</span>
                )}
              </div>
              {info.hasUpdate && info.manifestAvailable === false && (
                <div className="text-sm text-destructive">
                  更新清单不可达，下载可能失败：请先填写并保存更新代理后重试。
                </div>
              )}
              {info.hasUpdate && (
                <DemoAction>
                  {snapshot.phase === "readyToRestart" ? (
                    <Button size="sm" onClick={() => void restartNow()} disabled={restarting}>
                      {restarting ? <Loader2 className="animate-spin" /> : <RefreshCw />}
                      {restarting ? "正在重启…" : "重启以完成升级"}
                    </Button>
                  ) : (
                    <Button size="sm" onClick={() => openUpdateDialog()}>
                      <ArrowUpCircle />
                      立即升级
                    </Button>
                  )}
                </DemoAction>
              )}
              {info.releaseUrl && (
                <DemoAction><Button
                  variant="link"
                  size="sm"
                  className="h-auto p-0"
                  onClick={() => void openReleaseUrl(info.releaseUrl)}
                >
                  打开 GitHub Release
                </Button></DemoAction>
              )}
            </AlertDescription>
          </Alert>
        )}
      </CardContent>
    </SettingsGroup>
  );
}
