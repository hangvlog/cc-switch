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
