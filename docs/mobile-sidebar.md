# 手机侧栏与原 Codex 项目目录

3.19.27 的原桌面桥接在 `desktop/thread/list` 增加可选 `sidebar.version=1`、按桌面顺序排列的 `projects`，并为目录项提供 `name`、`projectId`、`isPinned`、`pinnedPosition`。协议仍是 desktop-owner v1；旧手机忽略新增字段，旧桌面返回的目录可继续读取。

优先读取 `state_5.sqlite` 的会话名称、项目表和项目根路径，项目按 `position` 排列。置顶兼容 `is_pinned` 和当前 `Pinned` 区段，顺序使用 `section_position`，排除归档和子代理任务。桌面迁移期间部分项目归属仍保存在 `.codex-global-state.json`，仅将当前会话的 local 项目归属映射到 SQLite 项目 ID，不返回全局状态或其他字段。读取上限 8 MiB，异常时保持数据库结果；不修改 Codex 文件。

手机保留完整目录，在界面上限制最近列表和各项目初始展开条数，避免较早的置顶会话被全局条数限制丢弃。电脑端的空项目也可显示。不读取或改写模型配置，发送协议与防重账本不变。

最小验证：桥接 catalog 测试覆盖改名、空项目顺序、旧置顶跨页、排除归档及全局敏感字段不外发；构建后通过真实中继核对电脑上的项目顺序和置顶顺序，再在手机原生界面验证。
