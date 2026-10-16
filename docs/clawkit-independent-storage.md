# ClawKit Desktop 独立数据目录（3.19.74）

默认数据目录为 `~/.clawkit`，数据库为 `clawkit.db`；Windows 示例为 `C:\Users\iie\.clawkit\clawkit.db`。设置、Skills 默认管理目录、应用备份、远程控制状态、日志和崩溃日志均在该目录；Tauri 自身的窗口与路径设置继续使用独立标识 `com.clawkit.desktop`。Codex/Claude 等被管理工具的自身目录、统一 Skills 目录及增强层已有独立账号存储不在本次迁移范围。

首次启动将原 `.cc-switch` 或旧 `app_config_dir_override` 指定的目录复制到临时目录。SQLite 使用只读源连接与 backup API 包含 WAL 数据，校验后发布为 `.clawkit`；其余文件复制为独立文件，内部链接实体化，外部/循环链接报错以便检查。原目录不删除、不降级、不写入应用配置。SQLite 可能为只读 WAL 连接维护共享锁状态，业务数据库及 WAL 内容不改写。

原版把 settings.json 固定写在默认 `.cc-switch`，因此旧自定义数据目录也从该默认路径复制设备设置。新自定义路径使用独立 Store 键 `clawkit_config_dir_override`，不能指向 CC Switch 目录。迁移失败时显示错误和重试入口，不创建空白数据库掩盖失败；已有 ClawKit 数据库/完成标记直接使用，不再次导入；未知非空目标目录拒绝覆盖。

云同步凭据、原远端目录等设置保留，但已继承的 WebDAV/S3 同步关闭（enabled/autoSync=false），避免迁移后后台覆盖 CC Switch 的云端数据。重新启用前选择独立远端路径，新配置默认 `clawkit-sync`。旧 Skills 对外链接不主动重写；ClawKit 后续启用/同步技能时使用新管理目录。两款工具仍可按用户操作修改同一个 Codex/Claude 原生配置，本次隔离的是两款管理器自身的数据。

验证覆盖：含 WAL 的 v20 快照、源数据库及设置未改、双向写入隔离、重复迁移、全新安装、旧自定义目录、损坏数据库失败与重试、目标冲突，以及前端目录/云同步设置。Windows 安装包需另行记录原生启动验收和摘要。

回退 3.19.73 会重新使用保留的 `.cc-switch`，不会自动获得 3.19.74 之后的新配置。回退前需另行备份 `.clawkit`，不要通过改数据库版本号或覆盖原文件进行回退。
