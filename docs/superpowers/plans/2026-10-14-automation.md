# P3 本地 API、CLI、MCP 与事件 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. 本次只交付计划；实施时遵循用户已授权的执行范围。

**Goal:** 让内部助手和脚本在显式授权下读取会议，并按阶段增加写入、录音和签名Webhook。

**Architecture:** Rust loopback gateway复用P0业务服务，独立TypeScript客户端包提供CLI与MCP；事件随业务提交并持久投递。

**Tech Stack:** axum候选、Tokio/sqlx、OS keychain、TypeScript Node客户端、官方MCP SDK。

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

- 撤销/关闭与已经在途的调用：新请求立即拒绝，监听/投递按状态停止。
- 浏览器跨Origin、伪造Host、过期discovery：不能绕过本地鉴权。
- 助手重复写请求：同幂等key不能启动两次录音或收费摘要。
- Webhook重试、宕机和多订阅：每订阅独立，不漏投、不串密钥。
- 恶意内容/巨大会议/任意保存路径：工具输出有限且不提供shell或系统文件通道。

## Task A1：gateway、客户端凭据和生命周期

**Files:** 新增 `automation/{mod,server,auth,discovery,types,commands}.rs`、设计中的 `20261014000400_add_automation.sql`、`database/repositories/automation.rs`、`frontend/src/components/AutomationSettings.tsx`；修改 `lib.rs`、state、Cargo依赖。

**Interfaces:** `AutomationServer::start(&self) -> Result<Discovery, EnhancementError>`、`stop()`；`ClientService::create(name,scopes,expires_at) -> ClientTokenOnce`、`revoke(id)`；固定protocol_version=1，`/api/internal/v1`路由。

- [ ] 写 `automation::tests::disabled_by_default_and_loopback_only`：未启用无listener/discovery；localhost服务成功；禁止0.0.0.0；退出清discovery。
- [ ] 写 `automation::tests::auth_revocation_origin_and_host`：无效/到期/撤销401，scope不足403，恶意Origin/Host拒绝；token不出现在文件/日志。
- [ ] `cargo test -p meetily --lib automation::tests` 先失败，再实现256bit token+hash、每请求检查、系统密钥库、限body/并发/速率，UI创建后仅显示一次token。
- [ ] 数据迁移包含clients/webhooks/events/deliveries及automation_requests；secret_ref真实keychain写入成功才提交，失败清理孤立secret。
- [ ] 验证第二次启动/占用端口/过期pid/强制退出恢复；提交 `feat: add opt-in authenticated local automation gateway`。

## Task A2：只读API与异步导出

**Files:** 新增 `automation/{routes,read_service,export_routes}.rs`；修改MeetingDocumentService/repositories只加分页读方法，不直接暴露Setting。

**Interfaces:** 设计的GET meetings/search/transcript/summary/jobs和POST exports/GET artifact download；`MeetingReadService::{list,search,transcript,summary}` 消费 `Page<T>` 和固定cursor。

- [ ] 写 `automation::tests::pagination_is_stable_and_search_is_bounded`：同created_at多meeting、251segments、删除某条后继续分页，不重复同条；q>256拒绝，snippet≤300，SQL参数化。
- [ ] 写 `automation::tests::read_scope_cannot_read_keys_audio_or_paths`：不存在settings/key/audio/任意path接口；恶意export body带path拒绝；artifact owner匹配。
- [ ] 写 `automation::tests::export_job_reports_committed_version`：202返回job，完成后artifact版本等于snapshot，源变更不混合；过期下载404。
- [ ] 各filter先失败后实现routes：现有LIKE搜索先复用，参数转义 `%`/`_` 的语义明确；若容量压力实测存在才加FTS。
- [ ] REST contract fixture与Rust/TS一致；官方接口兼容不作为验收；提交 `feat: expose bounded meeting reads and export jobs`。

## Task A3：CLI和MCP实接

**Files:** 新增 `automation-client/{package.json,tsconfig.json,src/{contracts,connection,http,cli,mcp}.ts,tests/{cli,mcp,connection}.test.ts}`、该包锁文件；新增 `docs/INTERNAL_AUTOMATION.md`。修改CI仅增加此包检查。

**Interfaces:** `AutomationClient::{listMeetings,searchMeetings,getTranscript,getSummary,exportMeeting,getJob}`；CLI `meetily-internal`与设计退出码；MCP六个read tools，不注册delete或shell。

- [ ] 用stub HTTP服务写 `connection_rejects_stale_instance_and_missing_token`、`cli_uses_stderr_and_exact_exit_codes`、`mcp_stdout_contains_only_protocol`；大结果分页不自动无限拉取。
- [ ] 在新包 `pnpm --dir automation-client test` 先失败，再实现连接、keychain/环境变量凭据、CLI/MCP DTO；MCP SDK版本和协议版本在fixture固定。
- [ ] 手动配置实际内部使用的一个助手，调用搜索→定位时间片段→摘要→导出，关桌面程序后明确不可用；配置只由用户选择，不自动覆盖外部配置。
- [ ] 凭据不在命令行argv/示例文档，实例退出码3，授权错误4；契约/类型检查通过；提交 `feat: add internal CLI and MCP meeting tools`。

## Task A4：独立write/record权限与幂等

**Files:** 新增 `automation/{write_routes,recording_routes,idempotency}.rs`；修改 `summary/commands.rs`、`audio/recording_commands.rs`、`lib.rs`中的录音封装，CLI可选命令和AutomationSettings。

**Interfaces:** 设计POST summary-jobs、cancel、recordings/start-stop；调用P0 JobService和共享 `RecordingService::start(request)->RecordingSession`、`stop(session_id)->RecordingStopResult`。桌面/tray/API共用一个状态机；不保留独立RECORDING_FLAG与API状态双写。

- [ ] 写 `automation::tests::scope_separation_and_idempotency`：read无法write/record；write无法录音；同key同body返回同session/job；同key不同body409。
- [ ] 写 `automation::tests::record_stop_cannot_stop_new_session`：旧session重复stop不能停刚开始的新录音；UI看到API状态，tray和设备一致。
- [ ] 写 `automation::tests::write_requires_expected_revision`：旧revision409，无provider调用；取消普通job需write，录音控制需record。
- [ ] 逐个filter先失败再实现；24h幂等记录和结果提交一致，避免“启动成功但响应超时再启动一次”；记录中不存正文/key。
- [ ] 真实麦克风/系统音、并发UI点击、设备断开、撤销client验证；提交 `feat: add scoped idempotent writes and recording control`。

## Task A5：事务事件与Webhook

**Files:** 新增 `automation/{events,outbox,webhook,signing,target_policy}.rs`、`frontend/src/components/WebhookSettings.tsx`；修改JobService/录音停止持久化/摘要提交事务，P4完成后接分离事件。

**Interfaces:** `EventWriter::append(tx, DomainEvent)`；`WebhookDispatcher::deliver_pending()`；`DomainEvent {id,meeting_id,kind,revision,job_id,occurred_at}`。五事件名与设计一致；`TargetPolicy::validate_and_resolve(url)` 返回固定连接IP列表。

- [ ] 写 `automation::tests::event_is_atomic_with_result`：rollback不产生完成event；recording.stopped不能替代transcript.completed。
- [ ] 写 `automation::tests::webhooks_retry_per_subscription_with_stable_event_id`：两个endpoint各自重试；rawbody不变、HMAC正确；宕机重启继续；关闭停止投递；初次+5次上限。
- [ ] 写 `automation::tests::target_policy_blocks_redirect_and_dns_rebinding`：metadata/link-local拒绝、DNS重新解析变化不可绕过、HTTP302不follow、localhost需显式测试许可。
- [ ] 先运行filter，再实现10s timeout、固定退避、2xx成功/4295xx重试/其他4xx终止、16KiB响应限制、正文禁日志；接收端示例按event_id唯一键落库。
- [ ] 用本地接收服务模拟超时/500/401/重复投递，记录签名读回与平台接收分别验收；提交 `feat: add durable signed meeting webhooks`。

**P3阶段完成条件：** A1–A3为只读首版8–12人日；A4–A5为5–8人日增量。speaker rename route等P4服务完成后再注册。源码默认关闭、实接助手、授权和撤销、事件接收都有证据。
