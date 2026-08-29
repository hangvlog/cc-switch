import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Check,
  Link2,
  LogOut,
  Play,
  Power,
  RefreshCw,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import { toast } from "sonner";
import { remoteApi, type CodexPlusPlusStatus } from "@/lib/api/remote";
import {
  remoteAccountApi,
  type RemoteAccountStatus,
} from "@/lib/api/remoteAccount";
import { AccountLoginCard } from "@/components/remote/AccountLoginCard";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { extractErrorMessage } from "@/utils/errorUtils";

type BridgeState = "idle" | "starting" | "waiting" | "connected" | "error";

export function RemoteControlPage() {
  const [account, setAccount] = useState<RemoteAccountStatus | null>(null);
  const [accountLoaded, setAccountLoaded] = useState(false);
  const [loginBusy, setLoginBusy] = useState(false);
  const [configurationBusy, setConfigurationBusy] = useState(false);
  const [state, setState] = useState<BridgeState>("idle");
  const [configurationReady, setConfigurationReady] = useState(false);
  const [canRollback, setCanRollback] = useState(false);
  const [remoteRunning, setRemoteRunning] = useState(false);
  const [peerOnline, setPeerOnline] = useState(false);
  const [plusPlus, setPlusPlus] = useState<CodexPlusPlusStatus | null>(null);
  const relaySocket = useRef<WebSocket | null>(null);
  const nativeUnlisten = useRef<UnlistenFn | null>(null);

  const closeSockets = useCallback(() => {
    const socket = relaySocket.current;
    relaySocket.current = null;
    socket?.close();
    nativeUnlisten.current?.();
    nativeUnlisten.current = null;
    setPeerOnline(false);
  }, []);

  useEffect(() => {
    void remoteAccountApi
      .status()
      .then((status) => setAccount(status.authenticated ? status : null))
      .finally(() => setAccountLoaded(true));
    void remoteApi
      .codexPlusPlusStatus()
      .then(setPlusPlus)
      .catch(() => null);
    void remoteApi
      .configurationStatus()
      .then((status) => {
        setConfigurationReady(status.configured);
        setCanRollback(status.canRollback);
      })
      .catch(() => null);
    void remoteApi
      .remoteStatus()
      .then((status) => setRemoteRunning(status.running))
      .catch(() => null);
    return closeSockets;
  }, [closeSockets]);

  const connectBridge = useCallback(async (websocketUrl: string) => {
    const relay = new WebSocket(websocketUrl);
    relaySocket.current = relay;
    nativeUnlisten.current = await listen<string>(
      "codex-remote-message",
      (event) => {
        if (relay.readyState === WebSocket.OPEN) {
          relay.send(
            JSON.stringify({ type: "relay.data", payload: event.payload }),
          );
        }
      },
    );
    relay.onopen = () => setState("waiting");
    relay.onmessage = (event) => {
      try {
        const message = JSON.parse(String(event.data));
        if (message.type === "relay.data") {
          void remoteApi.send(message.payload).catch(() => setState("error"));
        } else if (message.type === "relay.peer" && message.role === "mobile") {
          setPeerOnline(Boolean(message.online));
          setState(message.online ? "connected" : "waiting");
        }
      } catch (error) {
        console.warn("[RemoteBridge] invalid relay message", error);
      }
    };
    relay.onerror = () => setState("error");
    relay.onclose = () => {
      if (relaySocket.current === relay) setState("error");
    };
  }, []);

  const configure = useCallback(
    async (targetAccount = account) => {
      if (!targetAccount?.authenticated) return;
      setConfigurationBusy(true);
      try {
        const status = await remoteApi.configure();
        setConfigurationReady(status.configured);
        setCanRollback(status.canRollback);
        toast.success("Codex 配置已完成，重启 Codex 后生效");
      } catch (error) {
        setConfigurationReady(false);
        const status = await remoteAccountApi.status().catch(() => null);
        if (status && !status.authenticated) setAccount(null);
        toast.error(extractErrorMessage(error) || "Codex 配置失败");
      } finally {
        setConfigurationBusy(false);
      }
    },
    [account],
  );

  const rollbackConfiguration = async () => {
    setConfigurationBusy(true);
    try {
      const status = await remoteApi.rollbackConfiguration();
      setConfigurationReady(status.configured);
      setCanRollback(status.canRollback);
      toast.success("已恢复一键配置前的 Codex 配置");
    } catch (error) {
      toast.error(extractErrorMessage(error) || "恢复 Codex 配置失败");
    } finally {
      setConfigurationBusy(false);
    }
  };

  const startRemote = useCallback(async () => {
    if (!account?.authenticated || !configurationReady) return;
    setState("starting");
    closeSockets();
    try {
      const status = await remoteApi.startRemote();
      setRemoteRunning(status.running);
      const ticket = await remoteAccountApi.createSocketTicket();
      await connectBridge(ticket.websocketUrl);
    } catch (error) {
      setRemoteRunning(false);
      setState("error");
      toast.warning(
        extractErrorMessage(error) ||
          "手机远程连接暂不可用；一键配置和 Codex 桌面端使用不受影响",
      );
    }
  }, [account, closeSockets, configurationReady, connectBridge]);

  const login = async (username: string, password: string) => {
    setLoginBusy(true);
    try {
      const session = await remoteAccountApi.login(username, password);
      setAccount(session);
      toast.success("登录成功，正在一键配置 Codex");
      await configure(session);
    } catch (error) {
      toast.error(extractErrorMessage(error) || "登录失败");
    } finally {
      setLoginBusy(false);
    }
  };

  const stopRemote = async () => {
    closeSockets();
    await remoteApi.stopRemote();
    setRemoteRunning(false);
    setState("idle");
  };

  const logout = async () => {
    await stopRemote();
    await remoteAccountApi.logout();
    setAccount(null);
  };

  const launchCodex = async () => {
    try {
      await remoteApi.launchCodexPlusPlus();
      toast.success("ClawKit Codex 已启动，当前账号会自动复用");
    } catch (error) {
      toast.error(extractErrorMessage(error) || "启动 ClawKit Codex 失败");
    }
  };

  if (!accountLoaded) {
    return (
      <div className="px-6 py-8 text-sm text-muted-foreground">
        正在读取 ClawKit 登录状态…
      </div>
    );
  }

  if (!account) {
    return (
      <div className="mx-auto w-full max-w-3xl px-6 pb-8 pt-4">
        <AccountLoginCard busy={loginBusy} onLogin={login} />
      </div>
    );
  }

  const statusLabel = configurationReady
    ? "配置完成"
    : configurationBusy
      ? "正在配置"
      : "尚未配置";
  const displayName =
    account.user?.nickname || account.user?.username || "ClawKit 用户";

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 px-6 pb-8 pt-4">
      <Card className="overflow-hidden border-primary/25 shadow-sm">
        <div className="h-1 bg-gradient-to-r from-primary via-primary/60 to-transparent" />
        <CardHeader className="flex-row items-center justify-between space-y-0">
          <div className="flex items-start gap-3">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
              <Sparkles className="h-5 w-5" />
            </div>
            <div>
              <CardTitle className="text-lg">一键配置 Codex</CardTitle>
              <p className="mt-1 text-sm text-muted-foreground">
                已登录 {displayName}；账号模型、API
                网关和安全连接均由应用自动配置。
              </p>
            </div>
          </div>
          <Badge variant={configurationReady ? "default" : "secondary"}>
            {statusLabel}
          </Badge>
        </CardHeader>
        <CardContent className="space-y-5">
          <div className="flex items-center justify-between rounded-md border px-3 py-3">
            <div className="flex items-start gap-3">
              <ShieldCheck className="mt-0.5 h-5 w-5 shrink-0 text-emerald-500" />
              <div>
                <div className="text-sm font-medium">零侵入配置</div>
                <div className="text-xs text-muted-foreground">
                  仅定向更新 Codex 用户配置和模型目录；保留 auth.json、MCP、
                  Skills，不修改 Codex 应用、名称、图标或快捷方式。
                </div>
              </div>
            </div>
          </div>
          <div className="flex justify-between gap-2">
            <Button variant="ghost" onClick={() => void logout()}>
              <LogOut className="mr-2 h-4 w-4" />
              退出账号
            </Button>
            <div className="flex gap-2">
              {canRollback ? (
                <Button
                  variant="outline"
                  onClick={() => void rollbackConfiguration()}
                  disabled={configurationBusy}
                >
                  恢复配置
                </Button>
              ) : null}
              <Button
                onClick={() => void configure()}
                disabled={configurationBusy}
              >
                {configurationReady ? (
                  <RefreshCw className="mr-2 h-4 w-4" />
                ) : (
                  <Link2 className="mr-2 h-4 w-4" />
                )}
                {configurationBusy
                  ? "正在配置"
                  : configurationReady
                    ? "重新配置"
                    : "立即一键配置"}
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>
      <Card
        className={
          configurationReady
            ? "border-emerald-500/30 bg-emerald-500/5"
            : undefined
        }
      >
        <CardHeader>
          <CardTitle className="text-base">
            {configurationReady
              ? "配置已完成，现在启动 Codex"
              : "Codex 桌面增强"}
          </CardTitle>
        </CardHeader>
        <CardContent className="flex items-center justify-between gap-3 text-sm">
          <div className="flex items-start gap-3">
            {plusPlus?.installed ? (
              <Check className="mt-0.5 h-4 w-4 text-emerald-500" />
            ) : (
              <Power className="mt-0.5 h-4 w-4 text-muted-foreground" />
            )}
            <div>
              {plusPlus?.summary || "ClawKit Codex 增强层随安装包提供。"}
            </div>
          </div>
          {plusPlus?.installed ? (
            <Button
              size={configurationReady ? "default" : "sm"}
              onClick={() => void launchCodex()}
            >
              <Play className="mr-1.5 h-4 w-4" />
              启动 Codex
            </Button>
          ) : null}
        </CardContent>
      </Card>
      <Card>
        <CardContent className="flex items-center justify-between gap-3 py-4 text-sm">
          <div>
            <div className="font-medium">手机远程连接（可选）</div>
            <div className="mt-1 text-xs text-muted-foreground">
              仅启用此功能时才启动 Codex
              app-server；不可用时不影响一键配置和本机使用。
            </div>
          </div>
          <Badge variant={peerOnline ? "default" : "secondary"}>
            {peerOnline
              ? "手机已连接"
              : state === "error"
                ? "手机连接异常"
                : remoteRunning
                  ? "等待同账号手机"
                  : "未启用"}
          </Badge>
          {remoteRunning ? (
            <Button variant="outline" onClick={() => void stopRemote()}>
              <Power className="mr-2 h-4 w-4" />
              停止手机远程
            </Button>
          ) : (
            <Button
              variant="outline"
              onClick={() => void startRemote()}
              disabled={!configurationReady || state === "starting"}
            >
              <Link2 className="mr-2 h-4 w-4" />
              {state === "starting" ? "正在启用" : "启用手机远程"}
            </Button>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
