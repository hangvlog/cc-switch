import { ArrowRight, Check, ShieldCheck, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";

export function CodexQuickSetupHero({
  onConfigure,
}: {
  onConfigure: () => void;
}) {
  return (
    <section
      aria-labelledby="codex-quick-setup-title"
      className="relative overflow-hidden rounded-2xl border border-primary/25 bg-gradient-to-br from-primary/15 via-primary/5 to-background p-5 shadow-sm"
    >
      <div className="pointer-events-none absolute -right-16 -top-20 h-48 w-48 rounded-full bg-primary/15 blur-3xl" />
      <div className="relative flex flex-col gap-5 sm:flex-row sm:items-center sm:justify-between">
        <div className="max-w-2xl space-y-3">
          <div className="inline-flex items-center gap-1.5 rounded-full border border-primary/20 bg-background/70 px-2.5 py-1 text-xs font-medium text-primary">
            <Sparkles className="h-3.5 w-3.5" />
            新用户从这里开始
          </div>
          <div>
            <h2
              id="codex-quick-setup-title"
              className="text-xl font-semibold tracking-tight"
            >
              一键配置 Codex
            </h2>
            <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
              登录 ClawKit 后，自动配置可用模型、API
              网关和安全连接，无需手动填写 Base URL 或 API Key。
            </p>
          </div>
          <div className="flex flex-wrap gap-x-4 gap-y-2 text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1.5">
              <Check className="h-3.5 w-3.5 text-emerald-500" />
              自动获取账号模型
            </span>
            <span className="inline-flex items-center gap-1.5">
              <Check className="h-3.5 w-3.5 text-emerald-500" />
              配置完成直接启动
            </span>
            <span className="inline-flex items-center gap-1.5">
              <ShieldCheck className="h-3.5 w-3.5 text-emerald-500" />
              保留原配置与登录状态，不修改图标
            </span>
          </div>
        </div>
        <Button
          size="lg"
          onClick={onConfigure}
          className="h-12 shrink-0 px-6 text-base shadow-md shadow-primary/20"
        >
          立即一键配置
          <ArrowRight className="ml-2 h-4 w-4" />
        </Button>
      </div>
    </section>
  );
}
