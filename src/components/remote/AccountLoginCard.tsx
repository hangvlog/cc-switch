import { LogIn, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

export function AccountLoginCard({ busy, onLogin }: { busy: boolean; onLogin: () => Promise<void> }) {
  return <Card className="border-primary/25 shadow-sm">
    <CardHeader>
      <CardTitle className="text-lg">登录 ClawKit，连接你的电脑</CardTitle>
      <p className="text-sm leading-6 text-muted-foreground">使用现有 SSO 统一账号，手机和电脑登录同一账号即可查看和接续 Codex 任务。</p>
    </CardHeader>
    <CardContent className="space-y-4">
      <Button className="w-full" disabled={busy} onClick={() => void onLogin()}>
        <LogIn className="mr-2 h-4 w-4" />{busy ? "等待浏览器完成统一登录…" : "使用统一账号登录"}
      </Button>
      <p className="text-sm text-muted-foreground">将在浏览器打开统一登录中心，可使用手机号登录、注册或找回密码。完成后返回此处。</p>
      <p className="flex items-center justify-center gap-1.5 text-xs text-muted-foreground"><ShieldCheck className="h-3.5 w-3.5 text-emerald-500" />登录不会修改现有 Codex 模型配置</p>
    </CardContent>
  </Card>;
}
