# 远程助手统一登录与活动列表

2026-10-13：Mobile 0.1.34 / Desktop 3.19.52。

两端原来调用 `/auth/login`，复用中央用户库但没有统一登录页面。历史用户 ID 4 的 username / nickname 为 `hang`；不能把该名称当作独立账号体系或修改账号关联。本次源码将两端入口统一为“使用统一账号登录”，由系统浏览器打开既有 SSO 页面。手机号登录、注册、找回密码均由 SSO 处理。稳定身份使用中央 user ID / OIDC sub，设备 ID 不随重新登录改变。

`clawkit-remote` 是两端共享的远程助手产品公共客户端，没有客户端密钥。后端生成 5 分钟 PKCE S256 流程，浏览器仅收到 state 和 challenge；轮询凭据独立、哈希保存，不进入浏览器 URL。授权码单次兑换，账号结果单次领取。手机和桌面不收集密码。统一退出先撤销本机远程登录，再打开中央退出页面。当前部署使用单个后端 worker；进程重启中的未完成登录需要重试。

配置来源：`scripts/update_remote_sso.py --env-file <生产环境文件>`，幂等注册精确 HTTPS 回调和登出地址，保留其他客户端。发布前备份环境文件，不生成或轮换密钥。

## 活动视图

用户截图对应官方“活动视图”，不是普通最近列表。参考已安装 app-initial-6350d04672d0.js 的 Ykr / kei：等待回复、未读、运行中按优先级展示，其余近 7 日会话按活动时间分日，优先区在本次打开期间保持稳定。手机保留项目视图，活动视图增加优先级区、日期、行内项目名及两行最近回复。摘要来自原任务可见的 assistant 消息；禁止把工具输出或 analysis 用作列表摘要。

桌面桥接优先通过原桌面 IPC 获取运行状态、未读状态、可见回复和 heartbeat 通知状态。一次连接并发发现最多 32 个 owner，共用 1.2 秒截止时间，校验 owner/协议版本/会话 ID，读取后取消关注；不激活沉睡会话。离线摘要仅从身份校验后的原会话文件读取最多 512 KiB 尾部，返回最多 1200 字符。手机将常用 Markdown 显示为纯文本；没有引入官方完整 Markdown 解析器，复杂语法仍可能有显示差异。

定时任务过滤来自 `codex-dev.db` 的 inbox runs、未读 runs 和 active heartbeat 目标，按安装实例/官方账号迁移状态限定范围，只传布尔标记。不能用 `thread_source=automation` 替代该关联。用户来源的 heartbeat 会话如果当天未手动操作也会隐藏，等待处理除外。归档和子代理过滤、项目映射、已读同步沿用现有链路。

官方活动视图会保留其当前打开期间的条目，手机也保留自己打开期间的条目；两端打开时点不同的历史保留条目可能不同，不能宣称镜像同步了官方仅在内存中的完整活动视图。官方主动任务不因更新桥接被重启。

## REA 证据及边界

分析对象：`/Applications/ChatGPT.app` 26.1007.30912 (20156)，app.asar SHA-256 `75e397e2280dde93b70a85aa926f4282c728eacc84dfeb15ba7fb6ddd84628af`；renderer app-initial SHA-256 `604b5d2de4fbb41f3d0dc5b3662129c558e1c7b8aac8cc87225286f58dcfaf8f`。

- `pei/gei/mei/Sei/nj`：打开、关闭、清除、保留条目与内存快照。
- `Ykr/Xtr`：等待、未读、运行排序；`vF` 使用 recency。
- `Nei/Pei/n9n`：active heartbeat 与当天手动活动过滤；`adr/ycr` 使用 inbox/unread run 集合。
- 主进程 `SF/wF/CF` 与 `qn`：自动任务迁移范围及账号/用户/安装身份。
- 真实进程 PID 2126，engine PID 2660。关闭再打开活动视图后，优先区仅当前任务；已完成两项移入今天。修正 heartbeat 过滤后，前 20 项与官方 AX 顺序及日期边界一致。

使用 REA Skill 的证据方法及定向安装包/运行时工具，没有执行缺失的 REA 引擎。尚未证明官方窗口内存快照可跨端共享、所有隐藏根任务元数据完全可读，或 iOS 真机/APNs 已可用。不得用这次列表修复宣布全部 Remote 等价。

正式 Android 0.1.33 验收发现全量 405 会话首屏等待状态批次的延迟，0.1.34 按近期会话先查询并逐批呈现；继续扫描旧会话，完成整轮前不固定优先区快照。后台刷新不再反复显示首屏同步文案。


## 2026-10-13 发布与实测记录

| 组件 | 原始源码与完整提交 | 构建及目标 | 结果 |
|---|---|---|---|
| Android 0.1.34 | `hangvlog/chat-all` / `845c4c51a41bb560b0c48edff60e3afb685fba22` | D-GM12JWEF-1416，固定源码归档及锁文件；`expo prebuild --platform android --no-install`、`gradlew assembleRelease` | 自有发布服务 release 85 已发布；更新接口从 0.1.33 返回 0.1.34 |
| Desktop 3.19.52 | `hangvlog/cc-switch` / `b22e41d80729aad3266f75b7ca9dae864777f3a0` | Mac arm64 / enterprise Windows x64，`pnpm exec tauri build --no-bundle`；Launcher `65870636b3dccf7131fe8282802e8804886113ce` 打包 | 手动安装包可下载；缺少 updater 签名私钥，release 84 保持 draft，不作为自动更新完成 |
| SSO 后端 | `hangvlog/chat-all` / `be760a8f5074133aac0106ef244e77baa6c042fa` | 精确归档四个受影响源码文件；`py_compile`；`docker compose up -d --no-deps --force-recreate fastapi-app` | `fastapi-chat-app` 运行；真实 SSO start/callback/poll/device API 通过 |

后台依赖未变化，沿用镜像 `sha256:f291182e0e9070ed98812b2ee886636cf6b5ed53f9476f9b12d6f321b7b1e6d8`；源码 bind mount 部署哈希全部匹配。配置仅通过 `scripts/update_remote_sso.py` 注册 `clawkit-remote` 精确回调与退出地址，无新客户端密钥。容器未配置 Docker HEALTHCHECK，健康结论来自实际接口响应。

产物（自有发布服务，未使用 GitHub 传输）：

| 下载 | SHA-256 | 字节数 |
|---|---|---|
| [Android 0.1.34](https://clawkit.chat/api/releases/download/0.1.34/ClawKit-Mobile_0.1.34_android-universal-preview.apk) | `738eb49d8f4feeff090aa250b4342cd793dda664868c3db88fd31a0fa95ddc7f` | 46701829 |
| [macOS 3.19.52](https://clawkit.chat/api/releases/download/3.19.52/ClawKit-3.19.52-macos-arm64.dmg) | `38fe694430ec622f7bbfcb1fb0fd7c6447c6a4e3957f466266fcc3e0aeca28ba` | 51189444 |
| [Windows 3.19.52](https://clawkit.chat/api/releases/download/3.19.52/ClawKit-3.19.52-windows-x64.exe) | `7e27ab6cb0f364f1eca49de5d573bfc1b62074e6303e565fe67f8f50e6d4b10e` | 31486784 |

实测范围：

- Mobile 相关 48 项测试和 TypeScript 检查通过；桌面页面 10 项、bridge 定向用例及 cargo check 通过；SSO 隔离数据库用例通过。
- Android API 35 模拟器从 0.1.32 升 0.1.33，实际发起 SSO；同一授权流程通过 Mac 已登录 Chrome 完成后，移动应用自行领取会话，恢复已选电脑，未注入登录 token。随后覆盖升级 0.1.34，登录和设备选择保留。
- 0.1.34 正式包：活动优先区显示当前运行任务，今天前六项与对照相符，项目及回复摘要展示正常，后台刷新不再持续显示同步文案；设备列表明确显示 Mac 在线。此次没有制造新的未读完成事件，未读绿点和排序用例通过不等于本轮真机通知验收。
- macOS 3.19.52 从 DMG 安装，签名及二进制摘要匹配，真实窗口显示“手机已连接”；正式 bridge 经生产 relay 回读 405/405 状态。保留官方桌面及 engine 进程。
- Windows 3.19.51 → 3.19.52 静默覆盖升级成功，安装文件摘要、版本、快捷方式和卸载注册匹配，隔离配置进程启动存活。SSH session 0 无可交互窗口，未宣称 Windows GUI/SSO 点击流程通过。

Android 沿用相同 preview 签名（现有 debug certificate），不是商店正式签名；macOS 使用 ad-hoc 签名。物理 Android、iOS 真机和 APNs 未验收。当前官方会话启动时未加载 native engine adapter，列表和已有任务通道可用；手机新建及相应审批能力需要用户空闲后通过助手正常重开官方应用。本次未中断运行任务。

回滚：Mobile 保留 0.1.33 产物，Android 降级需按平台规则处理，不能保证无损覆盖；Mac 保留本机 `.local/mobile-sso-recents-20261013/rollback/ClawKit Desktop 3.19.51.app`；Windows 保留 `C:\self\hang\desktop-352\install-acceptance` 中旧安装目录、注册表和快捷方式；后端四文件与生产环境配置备份在 `/tmp/clawkit-remote-release-be760a8f/rollback`。

本机证据索引：`.local/mobile-sso-recents-20261013/` 下 `rea-evidence.json`、`installed-activity.json`、`mobile-034-acceptance.json`、`mobile-034-drawer.png`、`mobile-034-devices.png`、`windows-upgrade-result.json`、`backend-release.json` 和各产物下载校验记录。登录原始材料受本机权限保护，不纳入 Git 或发布包。

## 0.1.35 未发布及后续修正

0.1.35 已完成构建并保持 draft。它修正 Modal 生命周期和已读响应竞态，但正式 Android 包连接当前生产桌面时，真正未读任务的 markRead 未获确认。因此不能宣称 Desktop 3.19.52 的生产已读链路已经通过；早先已读任务返回 true 是提前返回，不能作为写入证明。

REA 对照确认，官方活动视图的 nj 是窗口内存状态，包含保留顺序和冻结的近期时间。两端各自重建即使使用同一目录，也会因打开时刻不同而分叉。0.1.36 / Desktop 3.19.53 改为手机请求官方窗口中的原始活动状态；打开、关闭、清除调用官方相同状态函数，手机不再单独排序。状态不可用时明确提示，不伪造一致的列表。

macOS 窗口接入通过继承的 Chromium debugging pipe，未开放 TCP 端口、未修改官方安装包、未写全局启动配置。仅允许活动 read/open/clear/close 和单个会话 markRead；手机不能发送 JavaScript。已读直接调用官方 threadReadState 服务并检查回执。适配先验证当前加载的两个 JS SHA-256，再选择已核对的函数；版本不匹配拒绝适配。Windows 尚未接入这条窗口状态通道。

已完成的开发验证：官方同版本隔离窗口 true unread → confirmed read；活动重复打开保留 activatedAt，清除、关闭/重开；真实隔离 engine 新建三条无消息会话后，官方优先区顺序与窗口状态接口一致。手机相关定向回归和 TypeScript 检查通过。正式产物和手机链路验收尚待后续记录，以上不等于生产或真机验收。

生产官方 PID 2126 仍保持运行，没有窗口私有管道。最终接入需要用户任务结束后正常退出官方应用，再从 ClawKit 打开；本次没有重启或中断生产任务。手机读取失败仍可打开对话，已读同步错误单独提示。

2026-10-13 安装包验收发现跨时区日期重分组问题：Mac Asia/Shanghai 的今天在 Android GMT 被重新算成昨天。0.1.37 / 3.19.54 将官方 relativeDay 的显示标签及原分组一起传递；手机不再按本地时区重分组。
