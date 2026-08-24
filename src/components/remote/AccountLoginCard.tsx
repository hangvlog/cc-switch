import { useState } from "react";
import { LogIn } from "lucide-react";
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
    <Card>
      <CardHeader>
        <CardTitle className="text-base">登录 ClawKit 账号</CardTitle>
        <p className="text-sm text-muted-foreground">
          手机与桌面登录同一账号后会自动发现并连接，无需配对码。
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
          {busy ? "正在登录" : "登录并自动连接"}
        </Button>
      </CardContent>
    </Card>
  );
}
