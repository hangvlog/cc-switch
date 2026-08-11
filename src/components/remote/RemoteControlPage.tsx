import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Check, Link2, LogOut, Power, RefreshCw, Settings2 } from "lucide-react";
import { toast } from "sonner";
import { remoteApi, type CodexPlusPlusStatus } from "@/lib/api/remote";
import {
  accountWebsocketUrl,
  clearRemoteAccount,
  createRemoteSocketTicket,
  loadRemoteAccount,
  loginRemoteAccount,
  saveRemoteAccount,
  type RemoteAccountSession,
} from "@/lib/api/remoteAccount";
import { AccountLoginCard } from "@/components/remote/AccountLoginCard";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { extractErrorMessage } from "@/utils/errorUtils";

type BridgeState = "idle" | "starting" | "waiting" | "connected" | "error";
const RELAY_URL_STORAGE_KEY = "codex-remote-relay-url-v1";

function configuredApiBase() {
  return (import.meta.env.VITE_CODEX_REMOTE_API_URL || "").replace(/\/$/, "");
}

function normalizeApiBase(value: string) {
  return value.trim().replace(/\/+$/, "");
}

function isApiBaseValid(value: string) {
  try {
    const url = new URL(normalizeApiBase(value));
    return ["http:", "https:"].includes(url.protocol) && Boolean(url.host);
  } catch {
    return false;
  }
}

export function RemoteControlPage() {
  const [apiBase, setApiBase] = useState(
    () => localStorage.getItem(RELAY_URL_STORAGE_KEY) || configuredApiBase(),
  );
  const [showAdvanced, setShowAdvanced] = useState(() => !isApiBaseValid(apiBase));
  const [account, setAccount] = useState<RemoteAccountSession | null>(loadRemoteAccount);
  const [loginBusy, setLoginBusy] = useState(false);
  const [state, setState] = useState<BridgeState>("idle");
  const [peerOnline, setPeerOnline] = useState(false);
  const [plusPlus, setPlusPlus] = useState<CodexPlusPlusStatus | null>(null);
  const relaySocket = useRef<WebSocket | null>(null);
  const nativeUnlisten = useRef<UnlistenFn | null>(null);
  const autoStartedToken = useRef<string | null>(null);

  const closeSockets = useCallback(() => {
    relaySocket.current?.close();
    relaySocket.current = null;
    nativeUnlisten.current?.();
    nativeUnlisten.current = null;
    setPeerOnline(false);
  }, []);

  useEffect(() => {
    void remoteApi.codexPlusPlusStatus().then(setPlusPlus).catch(() => null);
    return closeSockets;
  }, [closeSockets]);

  const connectBridge = useCallback(
    async (ticket: string) => {
      const relay = new WebSocket(accountWebsocketUrl(apiBase, ticket));
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
    },
    [apiBase],
  );

  const start = useCallback(async () => {
    if (!account) return;
    const endpoint = normalizeApiBase(apiBase);
    if (!isApiBaseValid(endpoint)) {
      toast.error("服务地址无效");
      return;
    }
    setApiBase(endpoint);
    localStorage.setItem(RELAY_URL_STORAGE_KEY, endpoint);
    setState("starting");
    closeSockets();
    try {
      await remoteApi.start();
      const ticket = await createRemoteSocketTicket(endpoint, account.token);
      await connectBridge(ticket);
    } catch (error) {
      setState("error");
      if ((error as Error & { status?: number }).status === 401) {
        clearRemoteAccount();
        setAccount(null);
      }
      toast.error(extractErrorMessage(error) || "远程服务启动失败");
    }
  }, [account, apiBase, closeSockets, connectBridge]);

  useEffect(() => {
    if (!account || autoStartedToken.current === account.token) return;
    autoStartedToken.current = account.token;
    void start();
  }, [account, start]);

  const login = async (username: string, password: string) => {
    const endpoint = normalizeApiBase(apiBase);
    if (!isApiBaseValid(endpoint)) {
      toast.error("服务地址无效");
      return;
    }
    setLoginBusy(true);
    try {
      const session = await loginRemoteAccount(endpoint, username, password);
      saveRemoteAccount(session);
      localStorage.setItem(RELAY_URL_STORAGE_KEY, endpoint);
      setAccount(session);
      toast.success("登录成功，正在自动连接手机");
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
    clearRemoteAccount();
    autoStartedToken.current = null;
    setAccount(null);
  };

  if (!account) {
    return (
      <div className="mx-auto w-full max-w-3xl px-6 pb-8 pt-4">
        <AccountLoginCard
          apiBase={apiBase}
          busy={loginBusy}
          showAdvanced={showAdvanced}
          onApiBaseChange={setApiBase}
          onToggleAdvanced={() => setShowAdvanced((value) => !value)}
          onLogin={login}
        />
      </div>
    );
  }

  const statusLabel = state === "connected" ? "手机已连接" : state === "waiting"
    ? "等待同账号手机" : state === "starting" ? "正在启动" : state === "error"
      ? "连接异常" : "已停止";

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 px-6 pb-8 pt-4">
      <Card>
        <CardHeader className="flex-row items-center justify-between space-y-0">
          <div>
            <CardTitle className="text-base">手机远程控制</CardTitle>
            <p className="mt-1 text-sm text-muted-foreground">
              已登录 {account.user.nickname || account.user.username}；同账号手机会自动连接。
            </p>
          </div>
          <Badge variant={peerOnline ? "default" : "secondary"}>{statusLabel}</Badge>
        </CardHeader>
        <CardContent className="space-y-5">
          <div className="flex items-center justify-between rounded-md border px-3 py-3">
            <div>
              <div className="text-sm font-medium">ClawKit 账号安全中继</div>
              <div className="text-xs text-muted-foreground">无需配对码，令牌仅用于创建一次性连接票据</div>
            </div>
            <Button variant="ghost" size="sm" onClick={() => setShowAdvanced((value) => !value)}>
              <Settings2 className="mr-1.5 h-4 w-4" />高级设置
            </Button>
          </div>
          {showAdvanced ? (
            <Input value={apiBase} onChange={(event) => setApiBase(event.target.value)} disabled={state !== "idle" && state !== "error"} />
          ) : null}
          <div className="flex justify-between gap-2">
            <Button variant="ghost" onClick={() => void logout()}><LogOut className="mr-2 h-4 w-4" />退出账号</Button>
            <div className="flex gap-2">
              {state !== "idle" ? (
                <>
                  <Button variant="outline" onClick={() => void start()} disabled={state === "starting"}><RefreshCw className="mr-2 h-4 w-4" />重新连接</Button>
                  <Button variant="destructive" onClick={() => void stop()}><Power className="mr-2 h-4 w-4" />停止</Button>
                </>
              ) : (
                <Button onClick={() => void start()}><Link2 className="mr-2 h-4 w-4" />启动连接</Button>
              )}
            </div>
          </div>
        </CardContent>
      </Card>
      <Card>
        <CardHeader><CardTitle className="text-base">Codex++ 增强层</CardTitle></CardHeader>
        <CardContent className="flex items-start gap-3 text-sm">
          {plusPlus?.installed ? <Check className="mt-0.5 h-4 w-4 text-emerald-500" /> : <Power className="mt-0.5 h-4 w-4 text-muted-foreground" />}
          <div>{plusPlus?.summary || "Codex++ 为可选增强；远程协议不依赖它。"}</div>
        </CardContent>
      </Card>
    </div>
  );
}
