import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Check, Link2, LogOut, Play, Power, RefreshCw } from "lucide-react";
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
  const [state, setState] = useState<BridgeState>("idle");
  const [peerOnline, setPeerOnline] = useState(false);
  const [plusPlus, setPlusPlus] = useState<CodexPlusPlusStatus | null>(null);
  const relaySocket = useRef<WebSocket | null>(null);
  const nativeUnlisten = useRef<UnlistenFn | null>(null);
  const autoStartedIdentity = useRef<string | null>(null);

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
    void remoteApi.codexPlusPlusStatus().then(setPlusPlus).catch(() => null);
    return closeSockets;
  }, [closeSockets]);

  const connectBridge = useCallback(async (websocketUrl: string) => {
    const relay = new WebSocket(websocketUrl);
    relaySocket.current = relay;
    nativeUnlisten.current = await listen<string>(
      "codex-remote-message",
      (event) => {
        if (relay.readyState === WebSocket.OPEN) {
          relay.send(JSON.stringify({ type: "relay.data", payload: event.payload }));
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

  const start = useCallback(async () => {
    if (!account?.authenticated) return;
    setState("starting");
    closeSockets();
    try {
      await remoteApi.start();
      const ticket = await remoteAccountApi.createSocketTicket();
      if (!ticket.websocketUrl) {
        throw new Error("远程服务返回的连接地址无效");
      }
      await connectBridge(ticket.websocketUrl);
    } catch (error) {
      setState("error");
      const status = await remoteAccountApi.status().catch(() => null);
      if (status && !status.authenticated) setAccount(null);
      toast.error(extractErrorMessage(error) || "远程服务启动失败");
    }
  }, [account, closeSockets, connectBridge]);

  useEffect(() => {
    if (!account?.authenticated) return;
    const identity = `${account.deviceId || "desktop"}:${account.expiresAt || "session"}`;
    if (autoStartedIdentity.current === identity) return;
    autoStartedIdentity.current = identity;
    void start();
  }, [account, start]);

  const login = async (username: string, password: string) => {
    setLoginBusy(true);
    try {
      const session = await remoteAccountApi.login(username, password);
      setAccount(session);
      toast.success("登录成功，模型与手机连接正在自动配置");
    } catch (error) {
      toast.error(extractErrorMessage(error) || "登录失败");
    } finally {
      setLoginBusy(false);
    }
  };

  const stop = async () => {
    closeSockets();
    await remoteApi.stop();
    setState("idle");
  };

  const logout = async () => {
    await stop();
    await remoteAccountApi.logout();
    autoStartedIdentity.current = null;
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
    return <div className="px-6 py-8 text-sm text-muted-foreground">正在读取 ClawKit 登录状态…</div>;
  }

  if (!account) {
    return (
      <div className="mx-auto w-full max-w-3xl px-6 pb-8 pt-4">
        <AccountLoginCard busy={loginBusy} onLogin={login} />
      </div>
    );
  }

  const statusLabel = state === "connected" ? "手机已连接" : state === "waiting"
    ? "等待同账号手机" : state === "starting" ? "正在配置模型"
      : state === "error" ? "连接异常" : "已停止";
  const displayName = account.user?.nickname || account.user?.username || "ClawKit 用户";

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 px-6 pb-8 pt-4">
      <Card>
        <CardHeader className="flex-row items-center justify-between space-y-0">
          <div>
            <CardTitle className="text-base">手机远程控制</CardTitle>
            <p className="mt-1 text-sm text-muted-foreground">
              已登录 {displayName}；账号模型、网关和手机连接均由应用自动配置。
            </p>
          </div>
          <Badge variant={peerOnline ? "default" : "secondary"}>{statusLabel}</Badge>
        </CardHeader>
        <CardContent className="space-y-5">
          <div className="flex items-center justify-between rounded-md border px-3 py-3">
            <div>
              <div className="text-sm font-medium">ClawKit 账号安全中继</div>
              <div className="text-xs text-muted-foreground">
                无需填写 Base URL 或 API Key；凭据仅注入后台 Codex 进程。
              </div>
            </div>
          </div>
          <div className="flex justify-between gap-2">
            <Button variant="ghost" onClick={() => void logout()}>
              <LogOut className="mr-2 h-4 w-4" />退出账号
            </Button>
            <div className="flex gap-2">
              {state !== "idle" ? (
                <>
                  <Button variant="outline" onClick={() => void start()} disabled={state === "starting"}>
                    <RefreshCw className="mr-2 h-4 w-4" />重新连接
                  </Button>
                  <Button variant="destructive" onClick={() => void stop()}>
                    <Power className="mr-2 h-4 w-4" />停止
                  </Button>
                </>
              ) : (
                <Button onClick={() => void start()}>
                  <Link2 className="mr-2 h-4 w-4" />启动连接
                </Button>
              )}
            </div>
          </div>
        </CardContent>
      </Card>
      <Card>
        <CardHeader><CardTitle className="text-base">Codex 桌面增强</CardTitle></CardHeader>
        <CardContent className="flex items-center justify-between gap-3 text-sm">
          <div className="flex items-start gap-3">
            {plusPlus?.installed
              ? <Check className="mt-0.5 h-4 w-4 text-emerald-500" />
              : <Power className="mt-0.5 h-4 w-4 text-muted-foreground" />}
            <div>{plusPlus?.summary || "ClawKit Codex 增强层随安装包提供。"}</div>
          </div>
          {plusPlus?.installed ? (
            <Button size="sm" onClick={() => void launchCodex()}>
              <Play className="mr-1.5 h-4 w-4" />启动 Codex
            </Button>
          ) : null}
        </CardContent>
      </Card>
    </div>
  );
}
