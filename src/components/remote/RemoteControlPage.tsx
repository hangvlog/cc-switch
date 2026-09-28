import { useCallback, useEffect, useState } from "react";
import {
  Copy,
  ExternalLink,
  UploadCloud,
} from "lucide-react";
import { toast } from "sonner";
import { remoteApi } from "@/lib/api/remote";
import {
  remoteAccountApi,
  type RemoteAccountStatus,
} from "@/lib/api/remoteAccount";
import { AccountLoginCard } from "@/components/remote/AccountLoginCard";
import { ClawkitConfigurationCard } from "@/components/remote/ClawkitConfigurationCard";
import {
  COMPATIBILITY_CODEX_ENDPOINT,
  SECURE_CODEX_ENDPOINT,
  endpointModeFor,
  type CodexEndpointMode,
} from "@/components/remote/CodexEndpointSelector";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { DesktopConnectionCard } from "@/components/remote/DesktopConnectionCard";
import { extractErrorMessage } from "@/utils/errorUtils";
import { copyText } from "@/lib/clipboard";
import { settingsApi } from "@/lib/api/settings";


export function RemoteControlPage() {
  const [account, setAccount] = useState<RemoteAccountStatus | null>(null);
  const [accountLoaded, setAccountLoaded] = useState(false);
  const [loginBusy, setLoginBusy] = useState(false);
  const [configurationBusy, setConfigurationBusy] = useState(false);
  const [diagnosticBusy, setDiagnosticBusy] = useState(false);
  const [diagnosticUrl, setDiagnosticUrl] = useState<string | null>(null);
  const [configurationReady, setConfigurationReady] = useState(false);
  const [configuredModel, setConfiguredModel] = useState("");
  const [selectedModel, setSelectedModel] = useState("");
  const [configuredEndpoint, setConfiguredEndpoint] = useState("");
  const [endpointMode, setEndpointMode] =
    useState<CodexEndpointMode>("compatibility");
  const [customEndpoint, setCustomEndpoint] = useState("");
  const [availableModels, setAvailableModels] = useState<string[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [canRollback, setCanRollback] = useState(false);
  const selectedEndpoint =
    endpointMode === "compatibility"
      ? COMPATIBILITY_CODEX_ENDPOINT
      : endpointMode === "secure"
        ? SECURE_CODEX_ENDPOINT
        : customEndpoint.trim();

  const restoreEndpointSelection = useCallback((baseUrl?: string) => {
    if (!baseUrl) return;
    const mode = endpointModeFor(baseUrl);
    setConfiguredEndpoint(baseUrl);
    setEndpointMode(mode);
    if (mode === "custom") setCustomEndpoint(baseUrl);
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
      .configurationStatus()
      .then((status) => {
        setConfigurationReady(status.configured);
        setCanRollback(status.canRollback);
        setConfiguredModel(status.model || "");
        if (status.models?.length) setAvailableModels(status.models);
        if (status.model) setSelectedModel(status.model);
        restoreEndpointSelection(status.baseUrl);
      })
      .catch(() => null);
  }, [restoreEndpointSelection]);

  const configure = useCallback(
    async (targetAccount = account) => {
      if (!targetAccount?.authenticated) return;
      setConfigurationBusy(true);
      try {
        const status = selectedModel
          ? await remoteApi.configure(selectedModel, selectedEndpoint)
          : await remoteApi.configure(undefined, selectedEndpoint);
        setConfigurationReady(status.configured);
        setCanRollback(status.canRollback);
        setConfiguredModel(status.model || "");
        setSelectedModel(status.model || "");
        setAvailableModels(status.models || []);
        restoreEndpointSelection(status.baseUrl);
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
    [account, restoreEndpointSelection, selectedEndpoint, selectedModel],
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
      setConfiguredEndpoint(status.baseUrl || "");
      restoreEndpointSelection(status.baseUrl);
      toast.success("已恢复一键配置前的 Codex 配置");
    } catch (error) {
      toast.error(extractErrorMessage(error) || "恢复 Codex 配置失败");
    } finally {
      setConfigurationBusy(false);
    }
  };

  const login = async (username: string, password: string) => {
    setLoginBusy(true);
    try {
      const session = await remoteAccountApi.login(username, password);
      setAccount(session);
      toast.success("登录成功，可启用手机远程");
      const options = await remoteApi.modelOptions().catch(() => null);
      if (options) { setAvailableModels(options.models); setSelectedModel(options.defaultModel); }
    } catch (error) {
      toast.error(extractErrorMessage(error) || "登录失败");
    } finally {
      setLoginBusy(false);
    }
  };

  const logout = async () => {
    await remoteApi.enableOwner(false);
    await remoteAccountApi.logout();
    setAccount(null);
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
      <div className="flex items-center justify-between text-sm"><span>已登录 {displayName}</span><Button variant="ghost" onClick={() => void logout()}>退出登录</Button></div>
      <DesktopConnectionCard />
      <details>
        <summary className="cursor-pointer text-sm text-muted-foreground">可选：使用 ClawKit 模型服务</summary>
      <ClawkitConfigurationCard
        displayName={displayName}
        configured={configurationReady}
        configuredModel={configuredModel}
        configuredEndpoint={configuredEndpoint}
        selectedModel={selectedModel}
        selectedEndpoint={selectedEndpoint}
        endpointMode={endpointMode}
        customEndpoint={customEndpoint}
        models={availableModels}
        busy={configurationBusy}
        modelsLoading={modelsLoading}
        canRollback={canRollback}
        onModelChange={setSelectedModel}
        onEndpointModeChange={setEndpointMode}
        onCustomEndpointChange={setCustomEndpoint}
        onConfigure={() => void configure()}
        onRollback={() => void rollbackConfiguration()}
        onLogout={() => void logout()}
      />
      </details>
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
