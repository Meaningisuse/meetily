# P0 基础与共享合同 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. 本次只交付计划；实施时遵循用户已授权的执行范围。

**Goal:** 建立可复现构建、事务化内容版本、一致性文档快照和受限后台任务，供其余四包复用。

**Architecture:** 先保留现有业务行为，再将必要读写逐项封装为共享服务；数据库与任务状态是所有入口的事实来源。

**Tech Stack:** Rust/Tauri、SQLite/sqlx、Tokio、React/TypeScript、pnpm、现有 Bun tests。

**Spec:** [内部增强版落地设计](../specs/2026-10-14-internal-enhancement-design.md)。必须同时阅读；本文都是未来实施任务，本次未执行。

## Global Constraints

- 内部自用，覆盖 Pro 核心能力，不做收费授权。
- 保留 Next.js 静态界面、Tauri 2/Rust、SQLite、Whisper/Parakeet、Ollama/内置 LLM/BYOK。
- 公共 DTO 使用 `snake_case`；领域层时间为 `i64` 毫秒；无时间保留 `null`。
- 已有 10 个迁移不可编辑、重排或删除。新功能不读取授权状态。
- 首版 macOS；Windows 增量适配单独验收。禁止恢复历史 Python/FastAPI 为运行前置。
- 各 task 先写行为测试并确认失败，再实现、验证；审阅后按 task 拆独立提交。当前交付不自动提交或发布。
- 下列命令以仓库根目录运行。P0 必须先补齐依赖；原生依赖失败属于环境阻塞，不能标成测试通过。

## Review Focus

- 带 WAL 的数据库异常退出：先保留恢复材料，禁止无备份删除 WAL。
- UI 未保存编辑与导出/助手读取：保存失败不得导出旧版冒充新版。
- 生成期间修改会议：旧来源结果不能覆盖新结果。
- 120分钟录音与慢转写：积压必须可恢复，内存不得无限增长。
- 旧摘要、缺时间、孤立历史记录：迁移后仍可读，不伪造时间或姓名。

## Task F1：锁定可复现构建基线

**Files:** 修改 `Cargo.toml`、`frontend/src-tauri/Cargo.toml`、`llama-helper/Cargo.toml`、`frontend/package.json`、`frontend/pnpm-lock.yaml`、相关 `.github/workflows/build*.yml`；新增 `rust-toolchain.toml`、`scripts/check-enhancement.sh`、`docs/ENHANCEMENT_BUILD_BASELINE.md`。

**Interfaces:** 产出从根目录运行的 `scripts/check-enhancement.sh`，失败即非0退出；全员和CI共用。记录 Node/pnpm/Bun/Rust/native依赖的已验证版本。

- [ ] 写 baseline check：metadata两包存在、patch/profile在workspace根生效、锁文件未变化、包内sidecar存在；一次干净安装作为行为验证，不写镜像配置测试。
- [ ] 复现当前 metadata 两条警告；区分“搬 patch 到根会真的改变依赖”和“删除未生效 patch”，先证明需要特定fork再锁 commit；不能机械搬移后误称无行为变化。
- [ ] 以当前 Rust 1.94.1、Node 24.14.0/pnpm 10.32.1 为本机候选，核对CI和锁文件后固定通过构建的版本；README 的 pnpm9.15.9 与当前版本差异要在此收敛。Bun用于现有测试，安装并锁经过测试的具体版本；当前没有Bun。
- [ ] 实现检查入口：frozen install、`bun test frontend/tests`、`pnpm -C frontend exec tsc --noEmit`、`pnpm -C frontend build`、`node --test .github/verify-portable-ggml.test.cjs`、`cargo test -p meetily --lib`；构建script包含helper和FFmpeg准备，最终 `pnpm -C frontend tauri:build`。
- [ ] 记录锁文件hash、clean checkout实测日志和安装包启动；CPU+Metal各跑一次。baseline未知失败修复只限构建前置，扩大行为修改另列任务。
- [ ] 审阅并提交 `build: establish reproducible enhancement baseline`。

## Task F2：迁移与共享文档快照

**Files:** 新增设计中的 `20261014000100_add_content_versions.sql`、`20261014000200_add_enhancement_jobs.sql`；新增 `meeting/{mod,document,error,normalize,service,commands}.rs`、`database/repositories/content_version.rs`、`frontend/src/types/enhancement.ts`；修改 `database/models.rs`、repositories/mod.rs、`lib.rs`、相关旧API写入包装。

**Interfaces:** `MeetingDocumentService::snapshot(&self, meeting_id: &str) -> Result<MeetingDocument, EnhancementError>`；`MeetingWriteService::save_summary_markdown(&self, meeting_id: &str, markdown: &str, expected_revision: i64) -> Result<SourceVersions, EnhancementError>`；notes/标题/转写更新采用同一版本合同。F2迁移同时加入canonical表，旧结果保持规范化回读，用户保存后写canonical列，不覆盖原编辑器JSON语义。

- [ ] 写 `meeting::tests::snapshot_reads_all_pages_and_legacy_content`：251条转写，最后一句出现，旧markdown/旧JSON两fixture内容一致，空时间仍null，未知speaker仍null。
- [ ] 写 `meeting::tests::write_rejects_stale_revision`：两个编辑者读revision0，首写成功为1，第二expected0返回CONFLICT且旧结果未变。
- [ ] 写 `database::tests::enhancement_migrations_preserve_existing_rows`：依次执行原10迁移、填存量内容、执行新迁移，文本/时间/notes/summary数量和内容保留，meeting cascade正常。
- [ ] `cargo test -p meetily --lib snapshot_reads_all_pages_and_legacy_content` 等逐个运行，先确认因缺实现失败；实现读取事务、版本更新、规范化和DTO，不遍历当前可视区。
- [ ] 把所有相关已有写入命令转给service，避免绕过revision；更新JSON合同fixture，TS/Rust解码同一示例。检查保留旧签名的调用方。
- [ ] 写前端 `native_boot_without_legacy_http_addresses` 回归：移除旧HTTP地址后会议/模型设置仍加载。把Sidebar旧地址门槛换native-ready状态并审计profile IPC；证实无调用后退役旧profile commands/helper和5167/8178 CSP许可。外部模型发现路径保持，直到有模型迁移能力。
- [ ] 运行上面3个tests和既有meeting/summary回归；审阅并提交 `feat: add versioned meeting document service`。

## Task F3：持久任务、取消与来源冲突

**Files:** 新增 `jobs/{mod,types,repository,scheduler,cancellation}.rs`；修改 `state.rs`、`lib.rs`、`summary/{service,commands}.rs` 和需要的repositories。

**Interfaces:** `JobService::enqueue(&self, kind: JobKind, meeting_id: &str, source_revision: i64, params: serde_json::Value) -> Result<JobSnapshot, EnhancementError>`；`get(job_id)`、`cancel(job_id)`；`JobKind { Summary, Export, Diarization }` 与 `EnhancementError` 分别在jobs/types.rs与meeting/error.rs定义。`JobContext { job_id,cancellation }` 与原摘要status做兼容投影，保留原summary事件。

- [ ] 写 `jobs::tests::one_heavy_job_and_bounded_queue`：同一时间running≤1，排队16后第17返回BUSY；并发请求不穿透锁。
- [ ] 写 `jobs::tests::restart_interrupts_running_without_auto_retry`：重启将running→interrupted；无网络provider请求。
- [ ] 写 `summary::tests::source_change_does_not_overwrite_new_summary`：生成中speaker/文本revision变化，旧结果只进备份/过期标记。
- [ ] 逐个运行对应 `cargo test -p meetily --lib <test_name>`，确认失败后实现状态机和取消传播；结果与succeeded同事务提交，失败保存error_code。
- [ ] 兼容前端原摘要进度，新增统一JobStatus展示；取消/崩溃后可重新提交，原可读结果保留。
- [ ] 回归并提交 `feat: add persistent bounded enhancement jobs`。

## Task F4：必要可靠性修整与长摘要

**Files:** 修改 `database/manager.rs`、`audio/transcription/worker.rs`、`audio/{pipeline,incremental_saver,recording_manager}.rs`、`summary/{processor,service}.rs`；新增 `database/recovery.rs`、`summary/token_budget.rs`、`docs/ENHANCEMENT_ACCEPTANCE.md`。

**Interfaces:** `RecoveryService::backup_before_recovery(db_path: &Path) -> Result<PathBuf, EnhancementError>`（异常恢复先停写/关闭连接保存DB/WAL/SHM）；`TokenBudget::split(segments, model_context, prompt_budget, output_budget)` 返回 `Vec<SummaryChunk>`（在token_budget.rs定义含segments与estimated_tokens）保持段顺序的chunks，超过预算显式失败；实时转写队列用有界channel，溢出区间写入补处理清单。

- [ ] 写 `database::tests::recovery_preserves_wal_before_mutation`：备份成功前不得删除WAL；备份写失败不得继续破坏；正常迁移用online backup另测。
- [ ] 写 `audio::tests::overflow_marks_saved_audio_for_catchup`：慢worker、队列满，回调不阻塞、原音频完整、待补区间持久化且补处理去重。
- [ ] 写 `summary::tests::chinese_budget_and_all_providers_chunk_consistently`：中文/英文模板、三种provider预算，单块不超上下文，尾段和行动项不漏。
- [ ] 先复现失败，再实现限界与恢复；无保存音频且溢出时明确录音错误/停止提示，不声称可补转写。
- [ ] 跑核心测试入口，做120分钟、设备断开、磁盘满、强制退出后的手工验收；记录RSS/队列峰值/恢复时间。
- [ ] 审阅并提交 `fix: protect recovery data and bound meeting processing`。

**P0完成条件：** 上述构建、数据回归、共享合同和可靠性证据齐备；单纯metadata通过不算P0完成。此包3–5人日只覆盖必要修整；若队列恢复需较大重构，先在baseline报告中明确增量估算。
