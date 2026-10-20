# 文档索引

## 现行文档（按当前代码核对）

| 文件 | 内容 |
| --- | --- |
| [`INTEGRATION.md`](INTEGRATION.md) | 架构分层、一次「插上模组 → 能上网」的数据流、数据表、扩展指南、配置分节、兼容红线 |
| [`BUILD.md`](BUILD.md) | 本机开发、OpenWrt SDK 构建（feeds rust/host 与宿主直编两条路）、CI、安装后验证、常见问题 |
| [`API.md`](API.md) | HTTP / WebSocket 契约：响应信封、鉴权、`modem_id` 语义、103 条 `/api/*` 接口全表（外加 2 个 WebSocket；由 `router.rs` 导出） |
| [`UPSTREAM_AUDIT.md`](UPSTREAM_AUDIT.md) | 三仓库源码审计、功能迁移矩阵（D1–D13 / P1–P12）、代码变更清单、测试结果、真机待办；追加审计 §7（上游对齐实改）、§8（OpenVohive QMI 会话边界与 TLV 编号裁决）、§9（Siriling 多驱动）、§11（luci-app-mt5700 与两份厂商 AT 手册：鼎桥信号/流量私有命令、FM350 双卡预设）；上游致谢清单（§10） |
| [`SUPPORT_MATRIX.md`](SUPPORT_MATRIX.md) | 92 型号 × USB/PCIe 形态 × 平台；厂商 × 平台的数据通道指令；支持程度五级（收录/识别/后端/自动化/实机）；验证边界 |
| [`DIAL_STATE_MACHINE.md`](DIAL_STATE_MACHINE.md) | 拨号生命周期审计与统一状态机：旧调用图、问题清单（含代码证据）、新架构、状态迁移表、入口归属、兼容性、测试与回滚 |

## 历史文档说明

2026-10-20 的提交 `66853b8`（"Delete docs directory"）删除了当时全部文档（5777 行）。
其中 `INTEGRATION.md` / `BUILD.md` / `API.md` 已在同日**按现行代码重写**，不是恢复旧文；
旧文描述的是删除前的实现，不能作为设计依据。

以下文件**不再维护**，其职责已被替代：

| 旧文件 | 替代 |
| --- | --- |
| `PORTING.md`（逐路由迁移状态） | `API.md` 的接口全表 + `UPSTREAM_AUDIT.md` 的迁移矩阵 |
| `PROGRESS.md` / `HANDOFF.md` | `CHANGELOG.md` 与 git 提交历史 |
| `PRIVACY.md` / `DIAL_AUDIT.md` | 脱敏要求见 `scripts/check-desensitize.sh`；拨号审计见 `UPSTREAM_AUDIT.md` §2.2 |

## 维护约定

- 改接口 → 同步 `API.md`（路由全表由 `router.rs` 导出，接口增删必须改）；
- 改构建/依赖/CI → 同步 `BUILD.md`；
- 改分层、数据流、数据表字段或扩展步骤 → 同步 `INTEGRATION.md`；
- 迁移结论与真机状态 → 只写 `UPSTREAM_AUDIT.md` / `SUPPORT_MATRIX.md`，不要在别处重复。
