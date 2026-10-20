# 文档索引

本目录在 2026-10-20 的提交 `66853b8`（"Delete docs directory"）里被整体删除，
当时删掉的是 `API.md`、`BUILD.md`、`DIAL_AUDIT.md`、`HANDOFF.md`、`INTEGRATION.md`、
`PORTING.md`、`PRIVACY.md`、`PROGRESS.md`、`UPSTREAM_AUDIT.md` 与 `example-config.toml`
（共 5777 行）。根 `README.md` 与部分源码注释仍引用这些文件名。

**本目录只保留与当前代码一致、且逐条核对过的文档**。历史文档没有恢复：它们描述的是
删除前的实现，恢复会让人把过时设计当成现状（任务红线明确禁止）。

## 现有文档

| 文件 | 内容 |
| --- | --- |
| [`UPSTREAM_AUDIT.md`](UPSTREAM_AUDIT.md) | 三仓库源码审计、功能迁移矩阵、代码变更清单、测试结果、真机待办 |
| [`SUPPORT_MATRIX.md`](SUPPORT_MATRIX.md) | 91 个型号 × 数据形态 × 平台，厂商 × 平台的数据通道指令，验证边界 |

## 已删除、且**不再作为设计依据**的文档

| 旧文件 | 现状 | 替代 |
| --- | --- | --- |
| `INTEGRATION.md` | 删除 | 目录结构与分层见各模块顶部 `//!` 注释；迁移结论见 `UPSTREAM_AUDIT.md` |
| `API.md` | 删除 | 接口契约以 `rust/src/http/router.rs` + `http/handlers/*` 为准 |
| `BUILD.md` | 删除 | 构建入口以根 `Makefile` 与 `.github/workflows/build.yml` 为准 |
| `PORTING.md` | 删除 | `UPSTREAM_AUDIT.md` §2 的迁移矩阵 |
| `PROGRESS.md` / `HANDOFF.md` | 删除 | 进度以 `CHANGELOG.md` 与 git 提交为准 |
| `PRIVACY.md` / `DIAL_AUDIT.md` | 删除 | 脱敏要求见 `scripts/check-desensitize.sh` 与日志模块注释 |

> 源码注释里出现的 `docs/INTEGRATION.md` 等引用保留原样：它们指向的是设计意图，
> 不是可点击的现状说明。
