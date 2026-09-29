# ClawKit Desktop 3.19.26 macOS 更新恢复

3.19.25 的本机修复包支持数据库 v18，但集成发布流水线关闭了 macOS job，线上 Tauri 清单只有 Windows。插件在比较版本后仍解析当前平台 URL，因此即使当前版本更新也会报 TargetNotFound。

3.19.26 由 hangvlog/CodexPlusPlus 集成工作流从固定的本仓提交构建，恢复 macOS ARM64/x64 的 DMG、签名 app.tar.gz 和发布登记。共用静态更新清单的三个平台必须同时完成，不允许只发布 Windows 后覆盖 macOS 清单。保留现有公钥，不轮换签名身份。

数据库逻辑与 3.19.25 相同，仅提升更新可见版本；已通过的 v18 迁移和会话游标测试见 clawkit-database-v18.md。实际更新验收要求从已安装 3.19.25 检查更新、下载验签、安装重启到 3.19.26，再检查显示最新，并确认数据库完整性和供应商配置保留。
