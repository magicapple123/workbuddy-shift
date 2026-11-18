import { useEffect, useState } from "react";
import { ArrowUpCircle, ExternalLink, Loader2, RefreshCw, Save } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import * as api from "@/lib/api";
import type {
  GithubConfig,
  UpdateInfo,
} from "@/lib/types";
import { GITHUB_RELEASE_URL, GITHUB_REPOSITORY_URL, openReleaseUrl } from "@/lib/update";
import { useUpdateState } from "@/hooks/use-update-state";
import { cn } from "@/lib/utils";
import { UpdateInstallDialog } from "@/components/update-install-dialog";
import { DemoAction } from "@/components/demo-action";
import { useAccountsStore } from "@/stores/accounts";
import { SettingsFieldRow, SettingsGroup } from "./rows";

export function useAuthFile(): string | undefined {
  return useAccountsStore((s) => s.status?.authFile);
}

/** 自动更新：检查公开 GitHub Releases 源 + 安装签名更新。 */
export function UpdateCard() {
  const version = useAccountsStore((s) => s.status?.version);
  // 阶段与进度来自 Rust 更新服务（托盘同源）：下载完成时按钮换成「重启以完成升级」。
  const snapshot = useUpdateState();
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [installOpen, setInstallOpen] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [githubConfig, setGithubConfig] = useState<GithubConfig>({});
  const [proxyUrl, setProxyUrl] = useState("");
  const [proxySaving, setProxySaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void api
      .getGithubConfig()
      .then((config) => {
        if (cancelled) return;
        setGithubConfig(config);
        setProxyUrl(config.proxy ?? "");
      })
      .catch((e) => {
        if (!cancelled) toast.error("更新配置加载失败", { description: api.asError(e) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function check() {
    setChecking(true);
    try {
      const r = await api.checkUpdate(proxyUrl, true);
      setInfo(r);
      if (!r.ok) {
        toast.error("检查更新失败", { description: r.message || r.error });
      }
    } catch (e) {
      toast.error("检查更新失败", { description: api.asError(e) });
    } finally {
      setChecking(false);
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

  async function saveProxy() {
    const value = proxyUrl.trim();
    if (value) {
      try {
        const parsed = new URL(value);
        if (!parsed.hostname || !["http:", "https:"].includes(parsed.protocol)) {
          throw new Error("unsupported proxy protocol");
        }
      } catch {
        toast.error("代理地址格式不正确，请填写 HTTP/HTTPS 地址，例如 http://127.0.0.1:7897");
        return;
      }
    }

    setProxySaving(true);
    try {
      const saved = await api.saveGithubConfig({ ...githubConfig, proxy: value });
      setGithubConfig(saved);
      setProxyUrl(saved.proxy ?? "");
      toast.success(value ? "更新代理已保存" : "已关闭更新代理");
    } catch (e) {
      toast.error("保存代理失败", { description: api.asError(e) });
    } finally {
      setProxySaving(false);
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
            placeholder="例如 http://127.0.0.1:7897"
            spellCheck={false}
            autoComplete="off"
          />
        </SettingsFieldRow>

        <div className="flex flex-wrap gap-2 border-b border-border/60 bg-muted/25 px-4 py-3 sm:px-5">
          <DemoAction><Button size="sm" variant="outline" onClick={() => void saveProxy()} disabled={proxySaving}>
            {proxySaving ? <Loader2 className="animate-spin" /> : <Save />}
            保存代理
          </Button></DemoAction>
        </div>

        <div className="flex flex-wrap gap-2 border-b-0 border-border/60 px-4 py-3 sm:px-5">
          <DemoAction><Button size="sm" variant="outline" onClick={check} disabled={checking}>
            {checking ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            检查更新
          </Button></DemoAction>
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
                {info.releaseName && <span className="text-muted-foreground"> · {info.releaseName}</span>}
              </div>
              {info.hasUpdate && (
                <DemoAction>
                  {snapshot.phase === "readyToRestart" ? (
                    <Button size="sm" onClick={() => void restartNow()} disabled={restarting}>
                      {restarting ? <Loader2 className="animate-spin" /> : <RefreshCw />}
                      {restarting ? "正在重启…" : "重启以完成升级"}
                    </Button>
                  ) : (
                    <Button size="sm" onClick={() => setInstallOpen(true)}>
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
        <UpdateInstallDialog open={installOpen} onOpenChange={setInstallOpen} />
      </CardContent>
    </SettingsGroup>
  );
}
