import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export const COMPATIBILITY_CODEX_ENDPOINT = "http://62.234.99.177:80/v1";
export const SECURE_CODEX_ENDPOINT = "https://api.clawkit.chat/v1";

export type CodexEndpointMode = "compatibility" | "secure" | "custom";

export function normalizeEndpointForComparison(value: string): string {
  return value.trim().replace(/\/+$/, "");
}

export function endpointModeFor(value: string): CodexEndpointMode {
  const normalized = normalizeEndpointForComparison(value);
  if (normalized === COMPATIBILITY_CODEX_ENDPOINT) return "compatibility";
  if (normalized === SECURE_CODEX_ENDPOINT) return "secure";
  return "custom";
}

export function isValidCodexEndpoint(value: string): boolean {
  try {
    const parsed = new URL(value.trim());
    return (
      (parsed.protocol === "http:" || parsed.protocol === "https:") &&
      Boolean(parsed.hostname) &&
      !parsed.username &&
      !parsed.password &&
      !parsed.search &&
      !parsed.hash
    );
  } catch {
    return false;
  }
}

interface CodexEndpointSelectorProps {
  mode: CodexEndpointMode;
  customValue: string;
  disabled?: boolean;
  onModeChange: (mode: CodexEndpointMode) => void;
  onCustomValueChange: (value: string) => void;
}

export function CodexEndpointSelector({
  mode,
  customValue,
  disabled,
  onModeChange,
  onCustomValueChange,
}: CodexEndpointSelectorProps) {
  const customInvalid = mode === "custom" && !isValidCodexEndpoint(customValue);

  return (
    <div className="space-y-3 rounded-md border px-3 py-3">
      <div>
        <div className="text-sm font-medium">模型服务地址</div>
        <div className="text-xs text-muted-foreground">
          默认使用兼容 HTTP；若请求一直停留在“思考中”，可切换安全 HTTPS。
        </div>
      </div>
      <Select
        value={mode}
        onValueChange={(value) => onModeChange(value as CodexEndpointMode)}
        disabled={disabled}
      >
        <SelectTrigger aria-label="模型服务地址">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="compatibility">兼容 HTTP（默认）</SelectItem>
          <SelectItem value="secure">安全 HTTPS</SelectItem>
          <SelectItem value="custom">自定义地址</SelectItem>
        </SelectContent>
      </Select>
      {mode === "custom" ? (
        <div className="space-y-2">
          <Label htmlFor="clawkit-codex-endpoint">自定义 HTTP/HTTPS 地址</Label>
          <Input
            id="clawkit-codex-endpoint"
            value={customValue}
            onChange={(event) => onCustomValueChange(event.target.value)}
            placeholder="https://gateway.example/v1"
            disabled={disabled}
            aria-invalid={customInvalid}
          />
          {customInvalid ? (
            <p className="text-xs text-destructive">
              请输入完整的 HTTP 或 HTTPS 地址，且不要包含账号、查询参数或片段。
            </p>
          ) : null}
        </div>
      ) : null}
      <p className="break-all font-mono text-xs text-muted-foreground">
        {mode === "compatibility"
          ? COMPATIBILITY_CODEX_ENDPOINT
          : mode === "secure"
            ? SECURE_CODEX_ENDPOINT
            : customValue || "尚未填写"}
      </p>
      {mode === "compatibility" ? (
        <p className="text-xs text-amber-600 dark:text-amber-400">
          HTTP 兼容模式不加密模型请求，仅在 HTTPS 无法连接的设备上使用。
        </p>
      ) : null}
    </div>
  );
}
