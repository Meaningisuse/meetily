# P4 录后说话人分离 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. 本次只交付计划；实施时遵循用户已授权的执行范围。

**Goal:** 在真实时间对齐和模型探针通过后交付批处理分离及会议内人工姓名管理。

**Architecture:** 模型只输出原录音时间的turns，共享service以version化run提交；原文、模型归属与人工覆盖分离，所有出口统一解析有效speaker。

**Tech Stack:** Rust音频管线、Whisper/Parakeet、sherpa-onnx优先候选、pyannote本地sidecar备选、SQLite、React。

**Spec:** [内部增强版落地设计](../specs/2026-10-14-internal-enhancement-design.md)。必须同时阅读；本文都是未来实施任务，本次未执行。

**执行优先级：** P0 → P3 → P4 → P1 → P2；P1/P2最后实施，编号仅作工作包标识。

## Global Constraints

- 内部自用，覆盖 Pro 核心能力，不做收费授权。
- 保留 Next.js 静态界面、Tauri 2/Rust、SQLite、Whisper/Parakeet、Ollama/内置 LLM/BYOK。
- 公共 DTO 使用 `snake_case`；领域层时间为 `i64` 毫秒；无时间保留 `null`。
- 已有 10 个迁移不可编辑、重排或删除。新功能不读取授权状态。
- 首版 macOS；Windows 增量适配单独验收。禁止恢复历史 Python/FastAPI 为运行前置。
- 各 task 先写行为测试并确认失败，再实现、验证；审阅后按 task 拆独立提交。当前交付不自动提交或发布。
- 下列命令以仓库根目录运行。P0 必须先补齐依赖；原生依赖失败属于环境阻塞，不能标成测试通过。

## Review Focus

- VAD删除长静音后时间：必须回映原录音，不累计错位。
- 无精确时间、短应答与交叠：明确unknown/overlap，不假装精确归属。
- 模型rerun和人工更正：旧结果保留，歧义cluster不得错误继承姓名。
- 重新转写与同时修改：不同revision的文本、覆盖不能混用。
- 长录音/模型缺失/取消：有限资源，失败不损坏旧转写或已有标签。

## Task D1：模型/时间探针与Go记录（3–5人日）

**Files:** 新增 `scripts/diarization-probe/{README.md,run_probe.py}`、`docs/DIARIZATION_SPIKE_RESULTS.md`、`frontend/src-tauri/tests/fixtures/diarization/README.md`；probe结果/真音频/模型加入gitignore。脚本在外部隔离环境，不加入app常驻Python后端。

**Interfaces:** probe报告JSON `{hardware,model_hash,runtime_version,audio_minutes,der,overlap_der,rtf,peak_rss_mb,cancel_latency_ms,license,download_conditions,packaging_bytes}`；标注RTTM和audio匿名sample_id关联。

- [ ] 准备设计中的10段≥90分钟中文/夹英语/噪声/1–6人数据、20%留出和120分钟压力录音，得到人工turn标注与可用于内部测试的授权。
- [ ] 优先跑sherpa-onnx官方Rust/C API离线样例，核对框架及每权重许可；若质量不通过，再评估pyannote独立进程的首次下载/运行/打包。
- [ ] 记录原硬件，明确定义不含overlap的DER为主门槛、250ms collar，另列含overlap的DER；目标DER≤20%、RTF≤0.5、RSS≤2GiB、取消≤3秒。不能只在排除噪声的小样本上报成功。
- [ ] 同时验证Whisper打开段/词timestamp的中文漏字回归、Parakeet是否能产真时间；forced alignment单独计时和资源。
- [ ] 形成Go/No-Go：模型、时间、许可、打包任一未通过则暂停D3–D5并修订估算；P3可继续；P4技术门槛失败时先记录No-Go与修订结论，再安排后置P1/P2，不把未通过的分离集成计为完成。固定模型hash、参数和算法版本后才正式依赖。
- [ ] 审阅提交探针脚本与无隐私汇总 `docs: record diarization feasibility gate`，不提交真会议/模型文件。

## Task D2：真实时间与公共映射

**Files:** 修改 `audio/transcription/{provider,worker,whisper_provider,parakeet_provider}.rs`、`whisper_engine/whisper_engine.rs`、`parakeet_engine/`对应引擎；新增 `audio/time_map.rs`、`20261014000450_add_transcript_timing.sql`，修改 `audio/{mod,pipeline,import,retranscription}.rs`、Transcript DTO。

**Interfaces:** `TimedText {text,start_ms,end_ms}`、`TimingPrecision {Exact,Approximate,Unavailable}`；TranscriptResult增加 `timed_segments: Vec<TimedText>, timing_precision: TimingPrecision`，保留text；`TimeMapSpan`按设计；`map_span(start_sample,end_sample) -> Vec<SourceSpan>` 返回原录音区间，跨跳跃不造连续区间。

- [ ] 写 `audio::time_map::tests::vad_gap_maps_to_original_clock`：原0–1s和10–11s拼为2s，第二packed1–2s必须映为原10–11s，跨区间拆分。
- [ ] 写 `audio::time_map::tests::resampling_does_not_accumulate_drift`：44.1/48k输入→16k，120分钟末尾偏差≤20ms；不用墙钟推导时长。
- [ ] 写 `transcription::tests::unavailable_timing_remains_unknown` 和真实timestamp engine fixture：Parakeet无时间不切字猜时间；Whisper开启timestamps后不能漏已有中文尾句。
- [ ] 先跑Rust filter，确认失败后实现映射贯穿实时/导入/重转写；provider时间精度按设计的transcripts.timing_precision落库，对旧时间标approximate，不误称exact。
- [ ] 三入口同样本核对试听定位，提交 `feat: preserve source audio timing through transcription`。若D1证明必须forced alignment，单列新增aligner模块、测试和性能验收后再开放精确归属。

## Task D3：版本化run和批处理engine

**Files:** 新增 `20261014000500_add_diarization.sql`、`diarization/{mod,types,engine,service,alignment,model_manager,commands}.rs`、`database/repositories/diarization.rs`；修改 `jobs/`、`lib.rs`、录音停止/导入/重转写完成入口、已验证模型runtime打包脚本。

**Interfaces:** `DiarizationEngine::run(&self, audio_path: &Path, options: DiarizationOptions, cancellation: CancellationToken) -> Result<Vec<SpeakerTurn>, EnhancementError>`；`DiarizationService::enqueue(meeting_id, expected_speakers) -> Result<JobSnapshot, EnhancementError>`；`DiarizationOptions {expected_speakers: Option<u8>,model_id: String}`，提供人数为1–10。

- [ ] 写 `diarization::tests::assignments_require_overlap_margin_and_exact_timing`：占比0.6/领先0.2阈值边界；短应答/交叠/approximate输出unknown或overlap；无LLM猜名。
- [ ] 写 `diarization::tests::failed_cancelled_or_stale_run_keeps_active_result`：失败/取消/提交前revision变化，active_run不切换，原文本和有效标签不变。
- [ ] 写 `diarization::tests::three_entrypoints_enqueue_after_audio_and_transcript_commit`：未保存音频NO_AUDIO；文本未完成NOT_READY；录后/导入/重转写各只排一个对应run。
- [ ] 先逐个filter失败，再实现按窗口推理/模型sha校验/离线缓存/队列/取消；staging→原子切active_run，与事件同事务。模型路径不接受HTTP任意参数。
- [ ] 模型首次下载完成才能标Ready，重启可加载；缺文件/损坏/用户关闭分离不会影响原转写；提交 `feat: add versioned offline diarization jobs`。

## Task D4：人工改名/合并/覆盖及重跑

**Files:** 新增 `diarization/speaker_service.rs`；修改repositories、MeetingDocumentService、summary/service缓存、import/retranscription兼容；新增 `frontend/src/components/Speakers/{SpeakerPanel,SpeakerEditor}.tsx`、`frontend/src/hooks/meeting-details/useSpeakers.ts`、`frontend/tests/hooks/speakers.test.tsx`。

**Interfaces:** `SpeakerService::{list,rename,merge,assign_segment}`；写接口携带expected_revision。`rename(meeting_id,speaker_id,display_name,expected_revision)`；`merge(meeting_id,source_id,target_id,expected_revision)`禁止跨会议/自合并/环；`assign_segment(meeting_id,segment_id,speaker_id,expected_revision)`写override并增speaker_revision。

- [ ] 写 `diarization::tests::manual_override_survives_same_transcript_rerun`：同revision重跑默认保留override；高交集匹配speaker才继承姓名，歧义新UUID。
- [ ] 写 `diarization::tests::retranscription_does_not_mix_old_overrides`：重转写备份旧内容、替换segment后旧覆盖不作用；失败回退旧可读结果。
- [ ] 写 `diarization::tests::rename_merge_resolve_consistently_and_stale_summary`：相同姓名不自动合并；无跨会/环；快照/API同名；导出一致性测试移至P2 E3；summary_stale=true，无自动付费请求。
- [ ] 先Rust tests失败，再实现effective speaker解析；Bun `speakers.test.tsx`测试试听5–10秒、改名失败保留输入、重跑清空覆盖需要确认。
- [ ] UI展示unknown/overlap而不是伪造speaker，期望人数1–10，取消后可以继续操作，姓名只在会议内生效；提交 `feat: add speaker correction and safe reruns`。

## Task D5：长会议、打包和内部验收

**Files:** 更新 `docs/DIARIZATION_SPIKE_RESULTS.md`、`docs/ENHANCEMENT_ACCEPTANCE.md`、操作文档；补跨入口必要回归tests，不复制实现逻辑当测试。

**Interfaces:** 验收记录与D1同字段，加source revision、run_id、安装包hash、操作结果；P3开放speaker rename，outbox开放diarization.completed。

- [ ] 用留出样本重复D1质量/性能门槛；实际120分钟录音测内存曲线、队列积压与时间映射，验证重任务不挤占录音。
- [ ] 包内离线执行模型，关网/关应用/模型损坏/磁盘满/取消/开新录音分别验证旧内容仍可读；缺音频给操作原因。
- [ ] 全链路：录后分离→试听→命名→合并/覆盖→重新摘要→MCP查询，核对所有姓名和source revision；导入/重转写也各跑一次。DOCX/PDF与MCP导出的一致性验收后置到P2 E3，不阻塞P4阶段完成。
- [ ] 运行P0统一测试入口、model回归和Tauri构建；使用代表听音验收，记录可接受的unknown比例与误归属样例。
- [ ] 审阅提交验收报告 `test: validate internal speaker workflow end to end`，发布安装包另按用户授权执行。

**P4完成条件：** D1Go记录+真时间+三入口+人工修正/版本+离线包内运行+听音效果都有证据；代码编译、推理返回turns、模型官网说明均不能代替实际说话人准确性验收。
