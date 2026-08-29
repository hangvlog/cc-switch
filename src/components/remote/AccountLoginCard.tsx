import { useState } from "react";
import { LogIn, ShieldCheck, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";

export function AccountLoginCard({
  busy,
  onLogin,
}: {
  busy: boolean;
  onLogin: (username: string, password: string) => Promise<void>;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");

  return (
    <Card className="border-primary/25 shadow-sm">
      <CardHeader>
        <div className="mb-1 flex h-10 w-10 items-center justify-center rounded-xl bg-primary/10 text-primary">
          <Sparkles className="h-5 w-5" />
        </div>
        <CardTitle className="text-lg">登录后自动完成 Codex 配置</CardTitle>
        <p className="text-sm leading-6 text-muted-foreground">
          使用 ClawKit 账号登录，应用会自动获取账号可用模型、API
          网关和安全连接，无需填写任何技术参数。
        </p>
      </CardHeader>
      <CardContent className="space-y-4">
        <Input
          aria-label="账号"
          value={username}
          onChange={(event) => setUsername(event.target.value)}
          placeholder="用户名 / 手机号 / 邮箱"
          autoComplete="username"
        />
        <Input
          aria-label="密码"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          placeholder="密码"
          type="password"
          autoComplete="current-password"
          onKeyDown={(event) => {
            if (event.key === "Enter" && username && password && !busy) {
              void onLogin(username, password);
            }
          }}
        />
        <Button
          className="w-full"
          disabled={!username.trim() || !password || busy}
          onClick={() => void onLogin(username.trim(), password)}
        >
          <LogIn className="mr-2 h-4 w-4" />
          {busy ? "正在登录并配置" : "登录并一键配置"}
        </Button>
        <p className="flex items-center justify-center gap-1.5 text-xs text-muted-foreground">
          <ShieldCheck className="h-3.5 w-3.5 text-emerald-500" />
          仅定向更新配置；保留登录状态，不修改 Codex 图标
        </p>
      </CardContent>
    </Card>
  );
}
