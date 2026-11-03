# ClawKit Desktop 3.20.0：合并上游 CC Switch v4.0.25

2026-11-03 把 `farion1231/cc-switch` main（`7afceec1`，v4.0.25）合并进 ClawKit
生产线（fork main `55966361`，3.19.84）。合并基点是 2026-08-24 的 `9a596158`，上游在此之后
有 1111 个提交、改动 1428 个文件；fork 自有 141 个提交、204 个文件。合并产生 31 个文件冲突。

## 取舍

| 范围 | 处理 |
| --- | --- |
| 数据库 schema、会话用量游标（`database/*`、`session_usage*.rs`） | 采用上游。fork 在这些文件里的改动全部是对上游 v18–v24 的回移植（`a1f8f279`、`dfb06e9f`、`81ebdf0f`）。迁移版本从 20 升到 24。 |
| 新版 schema 只读兼容层（`database/compatibility.rs`） | 删除。它只放行审计过的 v21–v24 数据库；应用本身迁移到 v24 后区间为空。高于 24 的数据库按上游规则进入"版本过新"恢复页。 |
| 独立存储（`~/.clawkit`、`clawkit.db`、首次复制迁移） | 保留。上游 v4 新增的设备本地状态（`live-state.json`、首写备份、旧接管备份）原本固定写 `~/.cc-switch`，改为 `config::get_device_dir()`（`~/.clawkit`），仍不跟随配置目录覆盖。界面里写死的 `~/.cc-switch` 路径文案同步改成 `~/.clawkit`。 |
| 界面 | 采用上游 v4 侧栏外壳。手机远控成为侧栏全局页 `remote`，页面常驻挂载，切走不打断登录、配对和中继；Codex 供应商页列表顶部保留一键配置卡片（`SwitchModePanel` 新增可选 `leadSlot`）。品牌文字通过 `PRODUCT_NAME` 和 `applyProductBrand` 统一，托盘菜单与托盘面板标题改为 ClawKit Desktop。 |
| 一键配置 Codex | v4 用"直连 / 路由 / 聚合"模式取代代理接管。配置前若处于路由或聚合模式，持 `lock_settled` 调用 `mode::controller::exit_locked` 退回直连；失败回滚或用户恢复时调用 `controller::enter` 回到原模式。备份清单新增 `previousStackMode`（缺省为路由），旧清单仍可恢复。 |
| 与 CC Switch 同机共存 | v4 启动时会按设备模式状态对齐各应用的客户端配置。ClawKit 首次迁移曾把 CC Switch 的 `live-state.json` 一并复制，且两边写同样的代理占位符，直接沿用会去接上 CC Switch 的路由（端口冲突后退回直连）并改写它正在管理的 Claude Desktop、Codex 等配置。处理：启动时把 3.20.0 之前留下的 `live-state.json` 改名为 `live-state.inherited-from-cc-switch.json`（标记 `clawkit-device-state-v1.json`，只做一次）；模式尚未落定且没有 ClawKit 自己的旧版接管备份时只记直连、不改写文件；客户端里的占位符只在本机库有旧版接管备份时才当作 ClawKit 的遗留物。 |
| GitHub workflow | 采用上游，`release.yml` 去掉推 tag 自动构建，`ci.yml` 去掉每周定时任务，并保留 `desktop-bridge` 契约测试。安装包仍在本机或受控 Windows 构建。 |

## 升级对用户数据的影响

- 已有 ClawKit 用户的 `~/.clawkit/clawkit.db`（v20）启动时由上游迁移链升到 v24，迁移前自动备份到 `~/.clawkit/backups/`。
- 3.19.80 起用兼容层打开的 v21–v24 数据库会直接按 v24 使用，不再降级或改写版本号。
- 回滚到 3.19.x 时，v24 数据库只能由 3.19.80–3.19.84 的兼容层打开；更早版本会进入"版本过新"恢复页。回滚前先保留迁移前备份。
