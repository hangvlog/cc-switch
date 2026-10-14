# macOS remote background service

Enabling phone remote installs a same-user launchd job and runs the existing original-owner relay without the Tauri window. SSO credentials, stable desktop identity, account-scoped queues and durable dispatch receipts keep their existing storage. No second Codex engine is started.

The job restarts on failure and starts at user login. Disable remote or sign out to stop it and remove its login registration. A file lock prevents duplicate daemons. Status uses a five-second heartbeat plus PID liveness. If launchd registration fails and no daemon is present, the existing in-process relay remains the fallback.

Installed executable commands: `--clawkit-remote-enable`, `--clawkit-remote-status`, `--clawkit-remote-disable`. The internal `--clawkit-remote-service <storage>` entry is dispatched before Tauri single-instance/window initialization. Mounted DMG paths cannot register a persistent service.

Ordinarily launched Codex continues to expose only the already supported owner IPC capabilities. Existing native engine and renderer adapters remain optional enhancement paths. This release does not claim ordinary-launch activity/read-state/full-event parity. No Windows or mobile packaging changes.

Validation: 12 RemoteControlPage tests passed; isolated compiled-binary launchd start, repeated enable, crash recovery and disable/removal passed. Rust unit tests could not run offline due to missing cached scc 2.4.0; dependencies were not installed. Final package and live relay evidence is recorded in chat-all docs/front/clawkit-desktop-3.19.56-release.json.

Rollback: disable the service, restore 3.19.55, then enable its in-process remote connection. Keep account, queue and receipt files.

## 3.19.59：后台预备普通启动的 IPC

远程后台服务定期检查当前用户的 IPC。已有可连接或仍被进程占用的端点保持原样；只有不存在或确认失效的旧端点才建立统一路由。旧失效 socket 改名保留。官方应用仍可从 Dock 或应用目录直接启动，不需要 ClawKit 发起启动、调试参数或 CLI adapter。

路由在后台服务存活期间保持待命；服务退出后，仍有官方客户端连接时继续转发，空闲后退出。官方进程身份通过系统 LOCAL_PEERPID 记录并核对可执行路径，避免只有路由运行就报告 Codex 可发送，也避免原有 lsof 路径检查只找到路由而漏掉真实官方窗口。

隔离实测：官方自行启动的实例，UI 标未读后外部清除广播未生效；预备路由后，同一个 profile 普通启动，未读和已读广播均持久化成功，官方版本检测也恢复正确。此功能只影响下次自然启动；不替换当前官方 socket，不中断用户任务。精确活动视图、新任务与完整事件流仍不是普通启动全功能等价。

### 长会话 IPC 响应

接收上限从旧的 32 MiB 对齐官方已安装版本的 256 MiB envelope；发送仍为 32 MiB，手机可见历史仍有独立条数和文本预算，不将整个大快照转发到手机。33 MiB 合法快照在旧版复现“Codex IPC 帧长度不兼容”，修复后同一测试通过；0 字节和超过官方上限的响应仍在分配正文前拒绝，并带字节数帮助定位。生产当前任务只读快照约 9.3 MB，未复现超限；用户报告的具体会话仍待核对，不能把上限差异认定为该次报错的唯一原因。

## 3.19.60：普通 owner 连续更新

手机0.1.44打开会话后，以独立watch租约订阅原owner的version11快照及Immer补丁。补丁严格检查owner、会话ID与前置revision，正文分段和socket超时不丢弃已接收片段；缺口或断线先标记状态未知，再从新快照恢复。不创建另一个引擎，不修改官方包。原始状态留在本机，仅推送已有的限量可见投影；内部状态变化不重复发送，突发变化合并为最新状态，租约90秒到期，同一任务不同手机互不取消。移动端保留8秒轮询兜底，并拒绝旧revision覆盖新状态。

此入口提供原owner可见状态流，不声称等同完整原始app-server事件日志；活动renderer视图与普通新任务接口仍有缺口。针对性测试覆盖分段超时、patch数组操作、revision缺口、可见投影去重、手机确认竞态与切换会话。正式安装/发布验收记录在chat-all。

## 3.19.61：手机执行记录不传完整脚本

普通命令记录仅投影状态、工作目录、输出、退出码和耗时，标题使用“执行命令”；MCP/动态工具执行记录移除输入 arguments。快照、历史、item 和 turn 事件共用输出投影。原生订阅响应去除已复制到 thread.turns 的 initialTurnsPage，避免未经精简的重复内容。待审批请求保留准确指令和权限信息。精简发生在本机向中继发送前，不改变官方本地 IPC 的状态快照。

3.19.62 的通知事件直接附带 threadTitle/projectName：按事件 threadId 查询本地索引并复用官方项目归属，不加载会话历史、不分页读取侧栏，也不向执行 owner 请求状态。普通启动与增强引擎提醒共用元数据补齐。
