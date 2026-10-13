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
          点击“打开 Codex”，启用连接，再用手机登录同一 ClawKit
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
            ? capabilities.nativeCreation
              ? "支持任务续聊、运行中追加指令和下一轮排队。"
              : "支持空闲任务续聊。"
            : "电脑任务连接尚未就绪，当前仅提供读取。"}
        </p>
        <p className="text-xs text-muted-foreground">
          {capabilities?.nativeCreation
            ? "手机可在电脑项目内新建对话，无需辅助功能权限。"
            : "如需手机新建对话，请先正常退出 Codex，再通过上方“打开 Codex”启动。已有任务不会自动中断。"}
        </p>
        <p className="text-xs text-muted-foreground">
          启用后会记住选择，应用启动时自动连接，切换页面不影响连接。电脑需保持开机、Codex
          和 ClawKit 需运行。
          {capabilities?.nativeCreation
            ? "支持的任务审批与澄清可在手机处理；身份验证仍需在电脑完成。"
            : "通过上方入口重新打开 Codex 后，可在手机处理支持的任务审批与澄清。"}
        </p>
      </CardContent>
    </Card>
  );
}
