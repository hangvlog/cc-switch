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
  const [launching, setLaunching] = useState(false);
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
  const launch = async () => {
    setLaunching(true);
    try {
      const next = await remoteApi.launchOriginal();
      setCapabilities(next);
      if (next?.activity?.ready) toast.success("Codex 已运行，活动同步已连接");
      else toast.warning("已发起 Codex 启动；活动同步状态会单独刷新");
    } catch (error) { toast.error(String(error)); }
    finally { setLaunching(false); }
  };
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">手机接续 Codex 原任务</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4 text-sm">
        <p className="text-muted-foreground">
          启用手机远程后，正常打开电脑上的 Codex，再用手机登录同一 ClawKit
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
            disabled={launching}
            onClick={() => void launch()}
          >
            {launching ? "正在打开…" : "打开 Codex"}
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
        <p className="text-xs text-muted-foreground" role="status">
          {capabilities?.activity?.ready ? "活动视图与已读同步已连接。" : `活动同步未就绪：${capabilities?.activity?.reason || "等待官方窗口接入"}。项目与历史仍可查看，可用的任务操作以连接状态为准。`}
        </p>
        <p className="text-xs text-muted-foreground">
          macOS 启动按钮不可用时，可在终端运行：
          <code className="block break-all">/Applications/ClawKit\ Desktop.app/Contents/MacOS/cc-switch --clawkit-open-codex</code>
          此入口用于打开 Codex 和活动视图同步，不加载设置界面。
        </p>
        <p className="text-xs text-muted-foreground">
          {capabilities?.nativeCreation
            ? "手机可在电脑项目内新建对话，无需辅助功能权限。"
            : status.ordinaryLaunchConfigured
              ? "普通启动已自动接入：当前任务完成后，正常退出并从 Dock 或应用程序重新打开 Codex，即可手机新建；无需经由 ClawKit 启动。"
              : status.ordinaryLaunchReason || "启用手机远程后会自动配置 macOS 普通启动入口；已运行的 Codex 不会被重启。"}
        </p>
        <p className="text-xs text-muted-foreground">
          {status.backgroundService
            ? "macOS 独立后台服务已启用：退出 ClawKit 窗口后仍保持连接，登录系统时自动恢复；停止手机远程或退出账号会关闭服务。电脑与 Codex 仍需运行。"
            : "启用后会记住选择，应用启动时自动连接，切换页面不影响连接。电脑、Codex 和 ClawKit 后台需保持运行。"}
          支持的任务审批与澄清可在手机处理；身份验证仍需在电脑完成。
        </p>
      </CardContent>
    </Card>
  );
}
