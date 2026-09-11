import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Check,
  Copy,
  ExternalLink,
  Link2,
  Play,
  Power,
  UploadCloud,
} from "lucide-react";
import { toast } from "sonner";
import { remoteApi, type CodexPlusPlusStatus } from "@/lib/api/remote";
import {
  remoteAccountApi,
  type RemoteAccountStatus,
} from "@/lib/api/remoteAccount";
import { AccountLoginCard } from "@/components/remote/AccountLoginCard";
import { ClawkitConfigurationCard } from "@/components/remote/ClawkitConfigurationCard";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { extractErrorMessage } from "@/utils/errorUtils";
import { copyText } from "@/lib/clipboard";
import { settingsApi } from "@/lib/api/settings";

type BridgeState = "idle" | "starting" | "waiting" | "connected" | "error";

export function RemoteControlPage() {
  const [account, setAccount] = useState<RemoteAccountStatus | null>(null);
  const [accountLoaded, setAccountLoaded] = useState(false);
  const [loginBusy, setLoginBusy] = useState(false);
  const [configurationBusy, setConfigurationBusy] = useState(false);
  const [diagnosticBusy, setDiagnosticBusy] = useState(false);
  const [diagnosticUrl, setDiagnosticUrl] = useState<string | null>(null);
  const [state, setState] = useState<BridgeState>("idle");
  const [configurationReady, setConfigurationReady] = useState(false);
  const [configuredModel, setConfiguredModel] = useState("");
  const [selectedModel, setSelectedModel] = useState("");
  const [availableModels, setAvailableModels] = useState<string[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
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
      .then((status) => {
        setAccount(status.authenticated ? status : null);
        if (!status.authenticated) return;
        setModelsLoading(true);
        void remoteApi
          .modelOptions()
          .then((options) => {
            setAvailableModels(options.models);
            setSelectedModel((current) =>
              options.models.includes(current) ? current : options.defaultModel,
            );
          })
          .catch(() => null)
          .finally(() => setModelsLoading(false));
      })
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
        setConfiguredModel(status.model || "");
        if (status.models?.length) setAvailableModels(status.models);
        if (status.model) setSelectedModel(status.model);
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
        const status = selectedModel
          ? await remoteApi.configure(selectedModel)
          : await remoteApi.configure();
        setConfigurationReady(status.configured);
        setCanRollback(status.canRollback);
        setConfiguredModel(status.model || "");
        setSelectedModel(status.model || "");
        setAvailableModels(status.models || []);
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
    [account, selectedModel],
  );

  const rollbackConfiguration = async () => {
    setConfigurationBusy(true);
    try {
      const status = await remoteApi.rollbackConfiguration();
      setConfigurationReady(status.configured);
      setCanRollback(status.canRollback);
      setConfiguredModel(status.model || "");
      setSelectedModel(status.model || "");
      setAvailableModels(status.models || []);
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
      if (!ticket.websocketUrl) {
        throw new Error("远程服务返回的连接地址无效");
      }
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

  const uploadDiagnostics = async () => {
    setDiagnosticBusy(true);
    try {
      const result = await remoteApi.uploadDiagnosticBundle();
      setDiagnosticUrl(result.url);
      await copyText(result.url);
      toast.success("诊断包已上传，7 天有效链接已复制");
    } catch (error) {
      toast.error(extractErrorMessage(error) || "诊断包上传失败");
    } finally {
      setDiagnosticBusy(false);
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

  const displayName =
    account.user?.nickname || account.user?.username || "ClawKit 用户";

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 px-6 pb-8 pt-4">
      <ClawkitConfigurationCard
        displayName={displayName}
        configured={configurationReady}
        configuredModel={configuredModel}
        selectedModel={selectedModel}
        models={availableModels}
        busy={configurationBusy}
        modelsLoading={modelsLoading}
        canRollback={canRollback}
        onModelChange={setSelectedModel}
        onConfigure={() => void configure()}
        onRollback={() => void rollbackConfiguration()}
        onLogout={() => void logout()}
      />
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
      <Card>
        <CardHeader>
          <CardTitle className="text-base">问题排查</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3 text-sm">
          <div>
            <div className="font-medium">一键上传诊断包</div>
            <div className="mt-1 text-xs leading-5 text-muted-foreground">
              仅上传脱敏后的 Codex 配置、模型目录和日志尾部，不包含账号会话、API
              Key、数据库或对话记录。链接 7 天有效，任何拿到链接的人均可访问。
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              onClick={() => void uploadDiagnostics()}
              disabled={diagnosticBusy}
            >
              <UploadCloud className="mr-2 h-4 w-4" />
              {diagnosticBusy ? "正在脱敏并上传" : "上传诊断包"}
            </Button>
            {diagnosticUrl ? (
              <>
                <Button
                  variant="ghost"
                  onClick={() =>
                    void copyText(diagnosticUrl).then(() =>
                      toast.success("诊断链接已复制"),
                    )
                  }
                >
                  <Copy className="mr-2 h-4 w-4" />
                  复制链接
                </Button>
                <Button
                  variant="ghost"
                  onClick={() => void settingsApi.openExternal(diagnosticUrl)}
                >
                  <ExternalLink className="mr-2 h-4 w-4" />
                  打开链接
                </Button>
              </>
            ) : null}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
