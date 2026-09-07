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

## Windows 手工验证

旧版 ClawKit Desktop 尚未包含下述状态同步修复时，先在 CC Switch 中只关闭 **Codex**
的本地代理接管，再退出 Codex，备份 `%USERPROFILE%\.codex\config.toml`，然后写入：

```toml
model_provider = "clawkit"
model = "gpt-5.6-sol"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.clawkit]
name = "ClawKit API"
wire_api = "responses"
requires_openai_auth = false
base_url = "https://api.clawkit.chat/v1"
experimental_bearer_token = "<bootstrap 返回的 API key>"
```

- `base_url` 必须是上面的纯 URL，不能写成 Markdown 链接文本。
- `experimental_bearer_token` 是登录后调用
  `/api/user/clawkit/codex/bootstrap` 返回的 `data.api_key`，不是 ClawKit 登录密码、账号
  Token 或 Codex 官方 OAuth Token。该值属于密钥；一旦贴到聊天、截图或日志中，应立即
  作废并重新生成。
- 手工验证可以先不配置 `model_catalog_json`；这不影响直接使用上面指定的模型，但 Codex
  的模型选择列表不会展示账号返回的完整模型集合。一键配置会额外生成
  `%USERPROFILE%\.codex\clawkit-models.json` 并设置 `model_catalog_json = "clawkit-models.json"`。
- 保存后完全退出并重新启动 Codex。若随后在旧版 CC Switch 中切换/保存 Codex 供应商或
  重新开启本地代理接管，CC Switch 仍可能按其当前供应商重新写入 Live 配置。

## 零侵入边界

- 一键配置只定向修改 Codex 用户目录内的 `config.toml`，并生成独立的
  `clawkit-models.json`；原有 MCP、Skills、审批策略及其他无关字段均保留。
- 一键配置与 CC Switch 使用同一份 Codex Live 配置。执行时会先关闭 Codex 的本地
  代理接管，再将 `clawkit` 同步为 CC Switch 本机设置、供应商数据库和 Live 配置中的
  当前供应商，避免退出恢复、应用重启或保存设置后被旧供应商覆盖。
- 上述处理只关闭 **Codex** 的代理接管，不会修改 Claude、Gemini 等其他应用的当前
  供应商或代理开关；之后仍可在供应商页面主动切换回其他 Codex 供应商。
- 写入前保留可恢复备份；写入任一步失败或用户点击回滚时，会同时恢复
  `config.toml`、模型目录、原 CC Switch 当前供应商和原 Codex 代理接管状态。
- 不修改或覆盖官方 Codex 的 `auth.json` 登录状态。
- 不修改官方 Codex 的应用图标、桌面快捷方式或名称。
- ClawKit 凭据存放在 `config.toml` 的专属 `[model_providers.clawkit]` 配置中，
  不进入渲染层、命令行参数或官方 `auth.json`。
- `.env` 不是 Codex 的配置文件；Gemini 等其他工具仍使用各自的配置机制。
- 停止手机远程或退出 ClawKit 账号不会启动、重命名或替换官方 Codex 应用。

ClawKit Desktop 重启时不会自动把其他供应商重新写回 `config.toml`。用户之后若主动在
供应商页面切换到其他 Codex 供应商，则按正常切换语义更新当前供应商；需要恢复 ClawKit
配置时再次点击“重新配置”即可。
