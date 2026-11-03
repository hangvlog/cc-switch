# 手机新建的 Claude 会话进入桌面列表（3.20.1）

问题：手机端在某个项目里新建 Claude 会话后，ClawKit 只把它记在自己的 `claude-sessions.json`，CLI 记录写在 `~/.claude/projects/` 下；Claude 桌面端的会话列表只读它自己的 `claude-code-sessions/<账号>/<组织>/local_<id>.json`，所以桌面上看不到。

处理：新建会话的第一轮 CLI 进程退出后（避免两个进程同时写同一份记录），桥接层在后台打开 `claude://resume?session=<CLI 会话 ID>`（macOS `open -g`，Windows `url.dll,FileProtocolHandler`，Linux `xdg-open`）。这是 Claude 桌面端自带的深链，CLI 的"在桌面继续"也用它；桌面调用自身的 `importCliSession`，写入 `local_<同一 ID>` 记录并把窗口切到该会话。结果以 `claude/event` 的 `desktopImport` 事件推给手机。

导入后手机仍持有 `cli:<ID>`：`index::resolve` 把 `cli:<ID>` 解析到 `cliSessionId` 相同的桌面会话，`claude/thread/read` 按请求的 ID 返回，避免手机当前视图失配；之后的消息按桌面会话处理（桌面已加载时送入桌面进程的收件口）。

已知行为：导入时桌面窗口会切到新会话；Claude 桌面端未运行时，深链会在后台启动它。

验证（2026-11-03）：单元测试覆盖"回合结束且进程退出后只请求一次导入"与 `cli:` 别名；iOS 模拟器里的 ClawKit 手机端 0.1.66 连接本机 3.20.1，在 `trace-collect` 新建会话，桌面日志 `Imported CLI session … as Desktop session local_…`，桌面会话库 53 → 54；导入后手机第二条消息由桌面进程回复，同一份记录无分叉。
