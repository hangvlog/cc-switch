# ClawKit Codex 一键配置

## 用户入口

应用首页首屏顶部会常驻显示“一键配置 Codex”引导卡，无论当前停留在哪个工具都能直接看到。点击“立即一键配置”会自动切换到 Codex 并进入配置流程；Codex 标题栏右侧也保留“一键配置”快捷入口。

## 使用流程

1. 输入 ClawKit 用户名、手机号或邮箱以及密码。
2. 点击“登录并一键配置”。
3. 应用自动获取当前账号可用模型、API 网关和凭据，定向更新 Codex 用户配置。
4. 页面显示“配置完成”后，点击“启动 Codex”。

已登录用户重新打开 ClawKit Desktop 时只恢复账号和配置状态，不会自动启动 Codex
或 Codex app-server。再次配置必须点击“立即一键配置”或“重新配置”；Codex 仅在
用户点击“启动 Codex”后打开。

手机远程连接是可选附加能力，只有点击“启用手机远程”后才会启动 Codex
`app-server`。若桌面版未携带 `app-server` 且没有安装全局 Codex CLI，页面只提示手机
远程暂不可用，不会回退已经成功的一键配置。

## 零侵入边界

- 一键配置只定向修改 Codex 用户目录内的 `config.toml`，并生成独立的
  `clawkit-models.json`；原有 MCP、Skills、审批策略及其他无关字段均保留。
- 写入前保留可恢复备份；写入任一步失败时回滚 `config.toml` 和模型目录。
- 不修改或覆盖官方 Codex 的 `auth.json` 登录状态。
- 不修改官方 Codex 的应用图标、桌面快捷方式或名称。
- ClawKit 凭据存放在 `config.toml` 的专属 `[model_providers.clawkit]` 配置中，
  不进入渲染层、命令行参数或官方 `auth.json`。
- ClawKit 网关当前通过 Chat Completions 上游提供模型能力，因此一键配置会沿用
  CC Switch 的兼容开关写入顶层 `web_search = "disabled"`，避免 Codex 默认携带
  上游不支持的托管搜索参数；恢复配置时该字段会随整份备份一并还原。
- `.env` 不是 Codex 的配置文件；Gemini 等其他工具仍使用各自的配置机制。
- 停止手机远程或退出 ClawKit 账号不会启动、重命名或替换官方 Codex 应用。

ClawKit Desktop 重启时不会自动把其他供应商重新写回 `config.toml`。用户之后若主动在
供应商页面切换到其他 Codex 供应商，则按正常切换语义更新当前供应商；需要恢复 ClawKit
配置时再次点击“重新配置”即可。
