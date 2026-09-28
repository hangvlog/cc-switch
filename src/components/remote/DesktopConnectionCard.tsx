import { useEffect, useState } from "react";
import { toast } from "sonner";
import {
  remoteApi,
  type DesktopRemoteStatus,
  type DesktopCapabilities,
} from "@/lib/api/remote";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

const labels: Record<string, string> = {
  disabled: "未启用",
  connecting: "连接中",
  waiting: "等待同账号手机",
  connected: "手机已连接",
  reconnecting: "正在重连",
  "login-required": "登录已过期，请重新登录",
};

export function DesktopConnectionCard() {
  const [status, setStatus] = useState<DesktopRemoteStatus>({
    enabled: false,
    phase: "disabled",
  });
  const [capabilities, setCapabilities] = useState<DesktopCapabilities | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let disposed = false;
    const refresh = async () => {
      try {
        const [next, capabilities] = await Promise.all([
          remoteApi.ownerStatus(),
          remoteApi.ownerCapabilities(),
        ]);
        if (!disposed) {
          setStatus(next);
          setCapabilities(capabilities);
        }
      } catch {
        /* Transient polling failures must not change the native enable preference. */
      }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 5000);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, []);
  const toggle = async () => {
    setBusy(true);
    try {
      setStatus(await remoteApi.enableOwner(!status.enabled));
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">手机接续 Codex 原任务</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4 text-sm">
        <p className="text-muted-foreground">
          在电脑打开 Codex，启用连接，再用手机登录同一 ClawKit
          账号。沿用原任务的模型、目录和权限，无需配置模型服务。
        </p>
        <div className="flex flex-wrap items-center gap-3">
          <Badge
            variant={status.phase === "connected" ? "default" : "secondary"}
          >
            {labels[status.phase] || "未启用"}
          </Badge>
          <Button disabled={busy} onClick={() => void toggle()}>
            {busy
              ? "正在处理"
              : status.enabled
                ? "停止手机远程"
                : "启用手机远程"}
          </Button>
          <Button
            variant="outline"
            onClick={() =>
              void remoteApi
                .launchOriginal()
                .catch((error) => toast.error(String(error)))
            }
          >
            打开 Codex
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          {capabilities?.build
            ? `已检测到 Codex ${capabilities.build}`
            : "尚未检测到受支持的 Codex 桌面进程"}
          。
          {capabilities?.canSend
            ? "支持空闲任务续聊。"
            : "未验证的版本仅提供读取，发送保持关闭。"}
        </p>
        <p className="text-xs text-muted-foreground">
          启用后会记住选择，应用启动时自动连接，切换页面不影响连接。电脑需保持开机、Codex
          和 ClawKit 需运行。任务审批请在电脑处理。
        </p>
      </CardContent>
    </Card>
  );
}
