# macOS 手机远控与官方 Codex 工具共存

ClawKit 3.20.2 的 stdio adapter 长期作为官方引擎的父进程。Codex 26.1111.21308
检查工具 peer、parent、grandparent 的签名身份与 Team ID。ClawKit 使用 ad-hoc
签名，工具发现因此返回 `-32603 Codex app tools pipe closed`。

Unix adapter 现在以 `CommandExt::exec` 交接为未修改的官方 Codex，保留 PID 和
官方桌面父进程。两条匿名管道由独立 relay 子进程连接桌面与引擎；手机 RPC、
事件、审批互斥和回执防重仍使用原有受限协议。

```text
官方 Codex 桌面应用
└── 官方 Codex 引擎（原 adapter PID，exec 交接）
    ├── 官方工具运行时
    └── ClawKit relay（手机与桌面收发）
```

没有修改官方应用、签名、校验模块或 socket 权限。远控本地通道保留单用户
0700 目录、0600 socket、单引擎锁和限定方法。可选远控 endpoint 无法绑定时，
relay 继续透明转发桌面的两个字节流。桌面 stdin 关闭后向引擎传递 EOF；
引擎 stdout 结束后 relay 清理通道并退出。exec 失败仅终止本次创建的 relay。
继承管道在 relay 内标记 close-on-exec，避免后续工具继承。

Windows 继续使用原启动实现；本轮真实验收范围为 macOS arm64 和上述官方版本。
官方协议、启动参数或安全规则变化时仍需要重新验证，不声称未来所有版本兼容。

相关验证入口：

```sh
cargo test --manifest-path src-tauri/crates/desktop-bridge/Cargo.toml --locked --offline native::
CLAWKIT_TEST_ADAPTER=/absolute/path/to/cc-switch node scripts/remote-official-tools-coexist.cjs
```

第二项启动官方应用的隔离 profile，通过继承调试管道使用其 app-server RPC，
验证真实 MCP 工具发现、远控 ready、model/list 和 engine/relay 父进程关系。
`CLAWKIT_EXPECT_REJECTION=1` 可用安装的旧版复现原 tools/list 拒绝。
隔离检查不替代安装后当前正式聊天的 Automations 新建/回读/删除验收，
也不替代手机实体设备网络及界面验收。

本轮保留既有 renderer 与 helper 的版本固定构建来源。正式 shell 必须通过
Tauri CLI 构建并运行现有 frontend 验证脚本，普通 cargo release 编译仅用于协议验证。
