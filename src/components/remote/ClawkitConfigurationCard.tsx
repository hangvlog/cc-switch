import { Link2, LogOut, RefreshCw, ShieldCheck, Sparkles } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { CodexModelSelector } from "@/components/remote/CodexModelSelector";
import {
  CodexEndpointSelector,
  type CodexEndpointMode,
  isValidCodexEndpoint,
  normalizeEndpointForComparison,
} from "@/components/remote/CodexEndpointSelector";

interface ClawkitConfigurationCardProps {
  displayName: string;
  configured: boolean;
  configuredModel: string;
  configuredEndpoint: string;
  selectedModel: string;
  selectedEndpoint: string;
  endpointMode: CodexEndpointMode;
  customEndpoint: string;
  models: string[];
  busy: boolean;
  modelsLoading: boolean;
  canRollback: boolean;
  onModelChange: (model: string) => void;
  onEndpointModeChange: (mode: CodexEndpointMode) => void;
  onCustomEndpointChange: (value: string) => void;
  onConfigure: () => void;
  onRollback: () => void;
  onLogout: () => void;
}

export function ClawkitConfigurationCard({
  displayName,
  configured,
  configuredModel,
  configuredEndpoint,
  selectedModel,
  selectedEndpoint,
  endpointMode,
  customEndpoint,
  models,
  busy,
  modelsLoading,
  canRollback,
  onModelChange,
  onEndpointModeChange,
  onCustomEndpointChange,
  onConfigure,
  onRollback,
  onLogout,
}: ClawkitConfigurationCardProps) {
  const endpointChanged =
    Boolean(configuredEndpoint) &&
    normalizeEndpointForComparison(selectedEndpoint) !==
      normalizeEndpointForComparison(configuredEndpoint);
  const modelChanged =
    Boolean(configuredModel) && selectedModel !== configuredModel;
  const configurationChanged = modelChanged || endpointChanged;
  const customEndpointInvalid =
    endpointMode === "custom" && !isValidCodexEndpoint(customEndpoint);
  const statusLabel = busy
    ? "正在配置"
    : configurationChanged
      ? "待应用"
      : configured
        ? "配置完成"
        : "尚未配置";

  return (
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
              已登录 {displayName}；账号模型、模型服务地址和连接参数均由应用配置。
            </p>
          </div>
        </div>
        <Badge
          variant={
            configured && !configurationChanged ? "default" : "secondary"
          }
        >
          {statusLabel}
        </Badge>
      </CardHeader>
      <CardContent className="space-y-5">
        <CodexModelSelector
          models={models}
          value={selectedModel}
          onValueChange={onModelChange}
          disabled={busy || modelsLoading || !models.length}
        />
        <CodexEndpointSelector
          mode={endpointMode}
          customValue={customEndpoint}
          onModeChange={onEndpointModeChange}
          onCustomValueChange={onCustomEndpointChange}
          disabled={busy}
        />
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
          <Button variant="ghost" onClick={onLogout}>
            <LogOut className="mr-2 h-4 w-4" />
            退出账号
          </Button>
          <div className="flex gap-2">
            {canRollback ? (
              <Button variant="outline" onClick={onRollback} disabled={busy}>
                恢复配置
              </Button>
            ) : null}
            <Button
              onClick={onConfigure}
              disabled={
                busy || modelsLoading || !selectedModel || customEndpointInvalid
              }
            >
              {configured ? (
                <RefreshCw className="mr-2 h-4 w-4" />
              ) : (
                <Link2 className="mr-2 h-4 w-4" />
              )}
              {busy
                ? "正在配置"
                : configurationChanged
                  ? "应用所选配置"
                  : configured
                    ? "重新配置"
                    : "立即一键配置"}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
