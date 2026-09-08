import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

interface CodexModelSelectorProps {
  models: string[];
  value: string;
  disabled?: boolean;
  onValueChange: (value: string) => void;
}

export function CodexModelSelector({
  models,
  value,
  disabled,
  onValueChange,
}: CodexModelSelectorProps) {
  return (
    <div className="space-y-2 rounded-md border px-3 py-3">
      <div>
        <div className="text-sm font-medium">默认模型</div>
        <div className="text-xs text-muted-foreground">
          默认优先选择 gpt-5.6-sol；重新配置后，新启动的 Codex
          会话使用所选模型。
        </div>
      </div>
      <Select value={value} onValueChange={onValueChange} disabled={disabled}>
        <SelectTrigger aria-label="默认模型" className="font-mono">
          <SelectValue placeholder="选择账号可用模型" />
        </SelectTrigger>
        <SelectContent>
          {models.map((model) => (
            <SelectItem key={model} value={model}>
              {model}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
