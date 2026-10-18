# macOS 普通启动的持久接入配置

当前普通启动接入由后台服务执行 `launchctl setenv`。该环境只在本次登录有效，登录恢复的 Codex 可能先于 ClawKit 服务启动，从而缺少新建通道。

2026-10-18 检查的官方版本为 26.1007.71326（20398）。其 macOS 主进程会先等待交互式登录 shell 完成环境采集，再选择 app-server 可执行文件。采集设置 `CODEX_SHELL=1`，通过 `_SHELL_ENV_DELIMITER_` 和 `command env -0` 读取结果，超时为 5 秒。默认本地主机配置是固定对象，没有发现可持久设置本地 `codex_cli_command` 的用户配置入口。

本配置脚本在 `.zshrc` 添加有标记的 source 块，生成独立、仅当前用户可读写的 zsh 配置。只有上述环境采集命令、手机远控启用、两个可执行文件存在且没有其他引擎覆盖时才设置现有 adapter 与官方引擎变量。普通终端和工具 shell 不设置这些变量；关闭远控后不再由此配置注入。仍保留旧版 launchctl 接入行为。

```sh
python3 scripts/configure-codex-shell-bridge.py install
```

适用于已安装 ClawKit Desktop、已启用手机远控且默认 shell 为 zsh 的 Mac。自定义配置目录使用 `--shell-dir`，其他存储位置使用 `--storage`。不修改官方 App，不启动窗口、不重启已有任务，不增加下载依赖。修改前保存私有备份；重复安装不重复插入块。

```sh
python3 scripts/configure-codex-shell-bridge.py uninstall
```

卸载仅删除自己的块及配置文件，保留其他 shell 设置。不覆盖第三方 CLI 配置、不替换软链接。它是独立配置修复，尚未集成进 ClawKit 安装器；其他设备需要执行一次安装命令。

已运行的 Codex 无法从这里更新已打开的 stdio，首次仍需用户在任务结束后完整退出并重开。之后配置不依赖 ClawKit GUI 或服务先启动；手机网络连接仍需既有后台服务在线。官方改变环境采集协议、用户 shell 耗时超过 5 秒或修改 shell 类型时需重新验证。活动分组等渲染层能力不属于此修复。

验证边界：清除继承变量后的真实 zsh 环境采集，以及现有正式 adapter 与官方 CLI 的隔离初始化、桥接新建/回读可单独验证；这些不等于原桌面重启、手机真实发送回复或界面验收。实际部署与残留验证项见主仓对应记录。
