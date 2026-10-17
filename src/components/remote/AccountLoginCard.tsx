import { useState } from "react";
import { LogIn, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

type LoginMode = "password" | "browser" | null;

export function AccountLoginCard({ busy, onLogin, onBrowserLogin }: {
  busy: LoginMode;
  onLogin: (username: string, password: string) => Promise<void>;
  onBrowserLogin: () => Promise<void>;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  return <Card className="border-primary/25 shadow-sm">
    <CardHeader>
      <CardTitle className="text-lg">登录 ClawKit，连接你的电脑</CardTitle>
      <p className="text-sm leading-6 text-muted-foreground">手机和电脑登录同一账号，即可查看和接续 Codex 任务。</p>
    </CardHeader>
    <CardContent className="space-y-4">
      <form className="space-y-4" onSubmit={async (event) => {
        event.preventDefault();
        if (busy || !username.trim() || !password) return;
        try { await onLogin(username.trim(), password); } finally { setPassword(""); }
      }}>
        <div className="space-y-2">
          <Label htmlFor="clawkit-login-account">账号</Label>
          <Input id="clawkit-login-account" autoComplete="username" placeholder="用户名、手机号或邮箱" required maxLength={100} disabled={!!busy} value={username} onChange={(event) => setUsername(event.target.value)} />
        </div>
        <div className="space-y-2">
          <Label htmlFor="clawkit-login-password">密码</Label>
          <Input id="clawkit-login-password" type="password" autoComplete="current-password" required maxLength={256} disabled={!!busy} value={password} onChange={(event) => setPassword(event.target.value)} />
        </div>
        <Button type="submit" className="w-full" disabled={!!busy || !username.trim() || !password}>
          <LogIn className="mr-2 h-4 w-4" />{busy === "password" ? "正在登录…" : "账号密码登录"}
        </Button>
      </form>
      <Button type="button" variant="outline" className="w-full" disabled={!!busy} onClick={() => void onBrowserLogin()}>
        {busy === "browser" ? "等待浏览器完成登录…" : "使用浏览器登录"}
      </Button>
      <p className="text-sm text-muted-foreground">需要注册或找回密码时，可使用浏览器登录中心。</p>
      <p className="flex items-center justify-center gap-1.5 text-xs text-muted-foreground"><ShieldCheck className="h-3.5 w-3.5 text-emerald-500" />登录不会修改现有 Codex 模型配置</p>
    </CardContent>
  </Card>;
}
