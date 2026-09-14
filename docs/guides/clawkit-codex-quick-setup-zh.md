# ClawKit Codex 一键配置

## 用户入口

应用首页首屏顶部会常驻显示“一键配置 Codex”引导卡，无论当前停留在哪个工具都能直接看到。点击“立即一键配置”会自动切换到 Codex 并进入配置流程；Codex 标题栏右侧也保留“一键配置”快捷入口。

## 使用流程

1. 输入 ClawKit 用户名、手机号或邮箱以及密码。
2. 点击“登录并一键配置”。
3. 应用自动获取当前账号可用模型和凭据，默认优先选择 `gpt-5.6-sol`，模型服务地址默认选择
   `http://62.234.99.177:80/v1` 兼容入口，定向更新 Codex 用户配置。
4. 登录后可切换账号支持的模型，也可在“模型服务地址”选择兼容 HTTP、安全 HTTPS 或自定义
   HTTP/HTTPS 地址，点击“应用所选配置”重新配置。
5. 页面显示“配置完成”后，点击“启动 Codex”。

在 Windows 系统代理失效或不可达时，获取模型和网关配置的 bootstrap 请求会自动绕过
系统代理直连重试一次，并保留账号认证和设备标识。两条网络路径都失败时，错误会显示实际
目标主机与连接类型；可在设置页上传脱敏诊断包继续排查。

已登录用户重新打开 ClawKit Desktop 时只恢复账号和配置状态，不会自动启动 Codex
或 Codex app-server。再次配置必须点击“立即一键配置”或“重新配置”；Codex 仅在
用户点击“启动 Codex”后打开。

手机远程连接是可选附加能力，只有点击“启用手机远程”后才会启动 Codex
`app-server`。若桌面版未携带 `app-server` 且没有安装全局 Codex CLI，页面只提示手机
远程暂不可用，不会回退已经成功的一键配置。

启动手机远程时会优先复用 Codex Desktop 写入 `config.toml` 的 `CODEX_CLI_PATH`。Windows
版 Codex Desktop 当前会把 CLI 放在 `%LOCALAPPDATA%\\OpenAI\\Codex\\bin\\<版本哈希>\\codex.exe`，
桌面端也会扫描这一版本化目录；macOS 同时兼容 `ChatGPT.app` 和历史 Codex 应用名称。仅在这些
Desktop 路径与常见全局安装目录均不存在时，才提示安装全局 Codex CLI。

## 零侵入边界

- 一键配置只定向修改 Codex 用户目录内的 `config.toml`，并生成独立的
  `clawkit-models.json`；原有 MCP、Skills、审批策略及其他无关字段均保留。
- 写入前保留可恢复备份；写入任一步失败时回滚 `config.toml` 和模型目录。
- 不修改或覆盖官方 Codex 的 `auth.json` 登录状态。
- 不修改官方 Codex 的应用图标、桌面快捷方式或名称。
- ClawKit 凭据存放在 `config.toml` 的专属 `[model_providers.clawkit]` 配置中，
  不进入渲染层、命令行参数或官方 `auth.json`。
- ClawKit 网关对 Responses 请求使用原生 `/v1/responses` 直通；一键配置不会新增或覆盖
  顶层 `web_search`，用户现有的搜索偏好会保留。
- `.env` 不是 Codex 的配置文件；Gemini 等其他工具仍使用各自的配置机制。
- 停止手机远程或退出 ClawKit 账号不会启动、重命名或替换官方 Codex 应用。

ClawKit Desktop 重启时不会自动把其他供应商重新写回 `config.toml`。用户之后若主动在
供应商页面切换到其他 Codex 供应商，则按正常切换语义更新当前供应商；需要恢复 ClawKit
配置时再次点击“重新配置”即可。

模型下拉框只展示 bootstrap 返回的当前账号可用模型。后端在写入前会再次校验所选模型，
不允许通过前端参数写入账号无权使用的模型。若未显式选择，则依次优先使用
`gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna`，再按内置兼容顺序回退。

## 模型服务地址

- **兼容 HTTP（默认）**：`http://62.234.99.177:80/v1`，供 TLS 无法建立的设备使用。
  HTTP 不加密 bearer token、提示词与模型响应，仅应在可信网络中使用。
- **安全 HTTPS**：`https://api.clawkit.chat/v1`。如果 HTTP 请求长时间停在“思考中”，且
  网关日志出现请求体 `unexpected EOF` 或 408，应切换到此入口并完全重启 Codex 后新建会话。
- **自定义地址**：接受带完整协议的 HTTP/HTTPS URL，可包含自定义端口和路径。地址不能包含
  用户名、密码、查询参数或 URL 片段；保存时会移除末尾 `/`。

当前选择会写入用户级 `config.toml` 的 `model_providers.clawkit.base_url`，再次打开配置页时
会从该字段还原选择。切换选项不会立即影响已运行的 Codex 进程，必须点击“应用所选配置”并
重启 Codex。
