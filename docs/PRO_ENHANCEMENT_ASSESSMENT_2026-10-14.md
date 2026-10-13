# Meetily 内部增强版：Pro 功能差距与改造评估

评估日期：2026-10-14（Asia/Shanghai）
代码基线：Community 0.4.1，commit `a2cb62e`。
用户确认范围：内部自用，参照 Pro 核心功能，不做收费授权。
文档状态：评估建议，未选定模型、未批准实施、未修改产品代码；不是已验收的实施设计。

## 1. 结论与依据

建议保留 Next.js 静态界面 + Tauri/Rust + SQLite 架构，在现有模块上增加四个能力：模板管理、文档导出、受控本地集成、录后说话人分离。不需要启用已归档的 `backend/` Python/FastAPI 服务。

官方 Pro 对比表主要差异为说话人识别、自定义摘要模板、高级导出、MCP/Webhooks/CLI。社区版现有源码已包含多模型摘要和模板基础能力，所以不能直接按官网的 Community 栏估算新增工作。

来源：[Pro 对比](https://meetily.ai/pro#pro-comparison)。官网介绍当前 Pro 1.11.0；本地社区版版本号为 0.4.1，二者不是同一代码线上的简单版本升级。未取得 Pro 私有源码，本评估针对公开功能的独立实现，不推断其内部算法。

## 2. 功能差距

| 能力 | 当前源码证据 | 必要改造 | 判断 |
|---|---|---|---|
| 本地转写 | Whisper、Parakeet 引擎及统一 Provider | 验证中文、长录音、设备切换与性能 | 复用 |
| 本地或 BYOK 摘要 | Ollama、内置 llama-helper、OpenAI/Claude/Groq/OpenRouter/自定义接口 | 上下文控制、密钥保护、带说话人内容一致性 | 复用并补强 |
| 六个内置模板 | `frontend/src-tauri/templates/` 中有六个 JSON | 验证打包发现与开发模式一致性 | 已有资源 |
| 自定义模板 | 用户目录优先加载、校验、列表、模板选择 | 创建/编辑/复制/删除/导入导出 UI 与完整写入接口 | 中等增量 |
| 高级导出 | 转写/摘要复制、BlockNote 到 Markdown 转换 | 统一内容快照、MD/TXT/JSON、DOCX/PDF、保存对话框与反馈 | 新模块，复用内容转换 |
| 说话人分离 | `speaker` SQL 字段存在，但主要模型与 API 无说话人对象；活动 worker 使用 `source: "Audio"` | 分离引擎、细粒度时间对齐、数据模型、名称/合并/逐行更正 UI | 最大新增模块 |
| MCP/CLI/Webhooks | 未发现活动的对应服务；`api_*` 当前是 Tauri commands | 本地 gateway、鉴权与权限、CLI/MCP 适配、事件可靠投递 | 新模块 |
| Windows GPU | 已有 Cargo 加速特性、构建脚本 | 编译与运行验收，新增分离模型的设备适配 | 不是从零新增 |

本次不纳入：试用与许可证、计费、厂商优先支持、Enterprise 组织模板库/SSO/集中管理。日历、自动入会、跨会议自动声纹认人也不属于这次对比范围。Pro 的“与会议聊天”可通过 MCP 助手读会议实现，不自动意味着需要在桌面端新增完整 RAG 聊天产品。

## 3. 说话人分离的关键范围修正

营销页说支持 live；操作文档明确说明实时录音的最终说话人标签在停止录音后产生。因此内部版优先实现录后批处理，覆盖录音、导入、重新转写三个入口；实时流式分离作为独立的后续需求。

依据：[官方说话人操作文档](https://docs.meetily.ai/features/speaker-diarization)。它还明确：需要保存音频；姓名按单次会议保存；不自动识别跨会议的相同人；可以听样本、重命名、合并、逐行纠错和指定预期人数后重跑。这里只采用这些公开行为，不复制界面细节。

### 3.1 时间对齐是主要技术风险

`frontend/src-tauri/src/audio/transcription/provider.rs` 的 `TranscriptResult` 只有 text/confidence/is_partial。worker 的起止时间是整个音频 chunk 的范围，Whisper 处理结果被合成字符串。Parakeet 也主要返回字符串。当前摘要入参仅拼接时间与文字。

一个 VAD chunk 内可能有多次说话人切换。不能按字符比例把文字切给不同人，也不能把 `mic/system` 当作多人身份。在重叠或短回复场景中，最长时间重叠映射只能作为明确标注的近似结果。

建议：

1. 扩展转写结果，保留带真实起止时间的细粒度 ASR segment；需要时接入时间对齐步骤。先验证 Whisper 的时间戳设置，避免恢复时间戳导致现有完整性问题回归。
2. 新建独立 `DiarizationProvider`：输入保存音频、预期人数等参数，输出按会议时间轴排列的 speaker turns。
3. 将 ASR segment 与 turns 对齐，保留 unknown/overlap 状态和人工更正；没有可靠细粒度时间时不得伪造精确归属。
4. 分离重跑只改说话人关联，保留原文；重新转写则生成新版本，避免旧校正静默套到新片段。

### 3.2 数据与 UI 改造

建议数据对象（概念草案，正式迁移前需要确定约束）：

- `meeting_speakers`：会议内稳定 speaker_id、显示名、样本音频范围。
- `speaker_turns`：音频时间范围、speaker_id、引擎与结果版本、重叠状态。
- transcript 归属：自动归属与人工覆盖分别保存；保留现有音源字段语义。
- 分离任务：状态、错误、进度、模型版本、音频指纹、结果版本。

增加 Speakers 面板：试听、命名、合并预览、逐行更正、重跑；合并应事务化，错误操作可恢复。说话人更正后摘要显示“来源已变化”，由用户重新生成；导出与 MCP 读取最新归属。

候选路径：

| 路径 | 优点 | 成本/验证项 |
|---|---|---|
| sherpa-onnx 的 Rust/C API | 官方提供离线分离和 Rust 示例，适合当前 Rust 桌面架构 | 原生库、ONNX Runtime 兼容、分割/embedding 模型组合、中文效果、跨平台打包需验证 |
| pyannote community-1 本地 Python 辅助进程 | 可作效果对照，模型支持离线运行及 exclusive diarization 输出 | Python/PyTorch 打包与平台适配较重；首次模型访问需接受条件；模型许可与署名要核对 |

来源：[sherpa-onnx 分离文档](https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/index.html)、[官方 Rust 示例](https://github.com/k2-fsa/sherpa-onnx/blob/master/rust-api-examples/examples/offline_speaker_diarization.rs)、[pyannote community-1 模型卡](https://huggingface.co/pyannote/speaker-diarization-community-1)。以上是待测试候选，不声称优于 Pro；框架、每个模型及打包原生库的许可需分别核对。新的本地辅助进程也不等于恢复归档的 FastAPI 后端。

先用 5–10 段人工标注的内部样本比较：双人采访、3–6 人讨论、远场、短回复、中英混合、重叠发言。记录 DER 的重叠/容差口径、人工修正量、实时因子、峰值内存。模型选型和指标门槛在样本评估后确定。

## 4. 自定义模板：补管理，不重写引擎

已有六个 JSON 文件：daily_standup、standard_meeting、project_sync、retrospective、sales_marketing_client_call、psychatric_session。编译内嵌 fallback 为两个，但打包资源会动态发现六个。六个文件已通过本次 JSON/基础字段检查，运行时发现尚未验证。

已有：`api_list_templates`、`api_get_template_details`、`api_validate_template`，以及 `useTemplates` 的选择 UI。details 目前只返回 section 标题，不能直接支持完整编辑。

改造点：

- 新增读取完整内容、保存、复制、删除接口；稳定 ID 与显示名分离，重命名不改变引用。
- 模板卡片、编辑器、导入/导出、冲突处理；内置模板只读，通过复制产生自定义模板。
- 保留 JSON 内部表示；需要贴近 Pro 的 Markdown 编辑和交换格式时，增加解析与渲染适配，而非更换摘要引擎。
- 限制模板 ID、路径和文件大小；原子写入，拒绝路径穿越；选中模板删除后明确回退。
- 复用已有 template/transcript fingerprint 缓存机制；补历史生成所用模板版本记录、持久化选择以及缓存失效回归测试。

Pro 参考：[模板文档](https://docs.meetily.ai/features/summary-templates)。内部版可以优先采用结构化表单；若要求兼容 Pro 模板文件，需单独约定 Markdown grammar 和往返保真范围。

## 5. 导出：共享一个内容快照

官方操作文档的范围比对比表标题更具体：摘要为 MD/DOCX/PDF，转写为 TXT/DOCX，保留时间与说话人；高级格式不代表所有内容都支持所有格式。[导出文档](https://docs.meetily.ai/features/export-and-copy)

建议由领域服务构建 `MeetingDocument` 快照，包含会议信息、完整转写、最新说话人名称和已保存摘要；MD/TXT/JSON、DOCX/PDF、CLI/MCP 导出共同使用。

主要改造：

- 从数据库读取完整转写，不导出当前界面加载的一页。
- 统一 Markdown、BlockNote、旧 JSON 三种摘要格式。当前 `useCopyOperations` 与 `blocknote-markdown.ts` 可复用，但不能长期依赖 React editor ref 作为唯一数据源。
- 未保存编辑明确提供保存后导出或导出编辑快照的行为；独立 CLI 默认读已保存版本，避免与界面产生不明差异。
- 增加保存对话框、文件名处理、原子输出、错误提示及打开输出目录。
- DOCX 保留结构化标题/表格；PDF 验证中文字体、分页、长表格、代码块和链接。转换方式选型要在目标机器完成生成/渲染探针；不依赖用户安装 Word，也不把 webview 截图当正式文档导出。

建议新增 `frontend/src-tauri/src/export/`，以及独立 ExportDialog；历史会议没摘要时仍允许导出转写。

## 6. 自动化：一个业务服务，三个入口

目前 `frontend/src-tauri/src/api/api.rs` 名字像 API，但活动接口是 Tauri commands。它们部分签名保留 `_auth_token` 兼容字段，不能把这个字段当作 HTTP 身份验证。需要新增真正的本地 gateway。

推荐结构：

```text
React/Tauri commands ─┐
本地 HTTP API ────────┼── 共享 Meeting/Recording/Summary 服务 ── SQLite/任务/文件
CLI → HTTP API ───────┤
MCP stdio → HTTP API ─┘
提交成功的业务事件 → outbox → 经允许的 Webhook
```

MCP、CLI 不应自行写 SQLite，不应另起第二套录音状态。独立客户端连接正在运行的桌面程序；程序退出时返回明确的不可用错误。

首批只读工具建议：list_meetings、search_meetings、get_transcript、get_summary、get_job；export_meeting随P2最后接入。列表/搜索返回有限摘要和可定位的 segment_id、时间、meeting_id，避免一次喂入全部会议。现有 LIKE 搜索可先复用，有实际容量压力后再评估 SQLite 全文索引；MCP 本身不要求引入向量数据库。

后续按独立权限增加：生成/取消摘要、修改说话人、开始/停止录音；不提供 MCP 删除会议工具。

必要控制：默认关闭、仅监听 loopback、每客户端凭据、read/write/record 权限、到期/撤销、桌面统一开关、受限并发与日志脱敏；拒绝未授权调用，访问层不能直接导出 API Key。通过一次手工 MCP 配置完成首版连接，“一键修改外部客户端配置”作为单独体验项评估。

参考：[官方集成](https://meetily.ai/mcp)、[API 架构](https://docs.meetily.ai/developers)、[鉴权与 scopes](https://docs.meetily.ai/developers/authentication)。我们建议复用业务能力，不承诺兼容 Pro 的全部 endpoint、token 规则或命令名。

### Webhook

首批仅需录音结束、转写完成、分离完成、摘要完成/失败事件。使用事件 ID、结果版本、HMAC 签名、持久化 outbox、超时、有限重试、投递历史和接收端幂等。事件优先只携带 ID 与状态，接收方再通过有权限的 API 读内容。

在任务结果提交后产生完成事件。重复回调不得重复生成笔记或事项；重新生成摘要要区分结果版本。发送目标需用户设置，目标校验覆盖 DNS/跳转地址，不允许应用被用来访问任意系统端点。

参考：[官方 Webhook 文档](https://docs.meetily.ai/developers/webhooks-and-sse)。官方支持比首版更多的事件和远程接入能力；LAN 配对、SSE 全量对齐与内置第三方连接器不纳入本次人日基线。

## 7. 支撑改造与代码落点

| 模块 | 主要现有位置 | 建议增加/调整 |
|---|---|---|
| 模板 | `summary/templates/`、`summary/template_commands.rs`、`hooks/meeting-details/useTemplates.ts` | 完整内容 CRUD、模板管理面板、选择持久化 |
| 导出 | `hooks/meeting-details/useCopyOperations.ts`、`lib/blocknote-markdown.ts`、MeetingDetails 组件 | `src-tauri/src/export/`、共享内容快照、导出 UI |
| 分离 | `audio/import.rs`、`audio/retranscription.rs`、`audio/transcription/`、`whisper_engine/`、`parakeet_engine/` | `src-tauri/src/diarization/`、时间对齐、任务、说话人 UI |
| 存储 | `database/models.rs`、repositories、migrations、`src/types/index.ts` | speakers/turns/覆盖/结果版本及向后兼容读写 |
| 摘要 | `summary/service.rs`、`processor.rs`、`useSummaryGeneration.ts` | 把人名与时间写入共享 transcript 格式，更新缓存与来源版本 |
| 集成 | `api/api.rs`、`audio/recording_commands.rs`、`summary/commands.rs` | 抽取可复用领域服务、新 `automation/` 与 CLI/MCP 客户端 |
| 生命周期 | `lib.rs`、state、tray | gateway 关闭、后台任务清理、统一事件 |

基础修整限定于这些模块所需的可靠性：

1. Cargo 的子包 patch/release profile 被忽略：当前 `cargo metadata` 已再次证实；确认配置意图并修正根 workspace 配置。
2. 建立可复现构建与统一测试入口，接入 PR 检查。
3. 数据库异常恢复先备份数据库/WAL/SHM；禁止丢失最近已保存内容来换取静默启动。
4. 音频与转写任务控制队列积压；分离任务与转写/摘要调度限制资源竞争，内存中不得无限堆积长录音。
5. Key/token 与诊断日志保护、长会议 token 预算与中文抽取验收。

## 8. 工作量与实施顺序

以下为粗估，基于熟悉 Rust/Tauri/React 的开发者；1 人日按 8 小时计。以 macOS 首版为估算假设，含模块测试与基本集成验证，未运行编译/模型探针，不能当作承诺排期。Windows、远程 LAN、广泛发行支持另外计算。

| 工作包 | 原工作包人日 | 可独立验收的交付 |
|---|---:|---|
| 可复现基线与必要可靠性修整 | 3–5 | 干净安装能构建，测试可运行，恢复与队列关键行为明确 |
| 本地只读 API + CLI + MCP | 8–12 | 鉴权、关闭/撤销、分页搜索、读取、手动连接一个目标助手；导出接入后置到P2，原预算随之重新分摊 |
| 写入/录音控制 + Webhook | 5–8 | 独立权限、状态一致、签名、重试与幂等 |
| 分离模型与时间对齐探针 | 3–5 | 样本比较、硬件性能/打包和时间对齐可行性结论 |
| 录后分离的完整集成 | 15–25 | 三入口、说话人管理、精确/不确定归属、重跑、摘要与导出一致 |
| 模板管理 | 3–5 | 六模板发现，完整 CRUD/复制/交换与重启持久化 |
| 高级导出 | 6–10 | 摘要 MD/DOCX/PDF，转写 TXT/DOCX，共享内容快照 |
| **全量核心范围合计** | **43–70** | 内部增强版，目标功能覆盖，不宣称效果与 Pro 相同 |

用户已调整顺序为 **P0基础 → P3自动化 → P4说话人分离 → P1模板管理 → P2高级导出**，P1/P2放到最后；编号仅标识工作包。P4探针尽早发现风险，正式集成以探针通过为前提。P3首版不包含export命令/工具，导出接入与跨出口姓名一致性验收随P2最后补齐；摘要生成沿用现有模板，不依赖P1管理界面。全量仍粗估43–70人日；原阶段人日拆分不再适用，各阶段预算在P0/P4探针后校准。

全量约 9–14 个全职开发周；多人开发需先固定内容/说话人/任务接口，不能简单除以人数。若同时交付 Windows，另预留约 8–15 人日做原生库、安装包、设备、字体与回归适配；这是待实测调整的估算。

如果要求录音中实时出现稳定 speaker labels、跨会议自动识别同一人、原生聊天 UI、Pro API 完整兼容或 LAN 远程录音，应另立需求并重估。

## 9. 验收清单与证据边界

- 模板：创建/复制/改名/删除/交换/重启后引用正确；编辑模板不静默改写历史摘要。
- 导出：长会议不缺页，中文 PDF 无缺字，DOCX 表格结构正确；未保存内容处理清晰，导出失败无半成品。
- MCP/CLI：默认关闭；错误凭据/权限拒绝；撤销生效；检索返回可定位片段；不会读出密钥。
- Webhook：结果写入成功后发送；接收端重复投递不重复落库；关闭或撤销停止后续发送。
- 分离：三入口均可处理；无音频返回明确限制；重跑不改原文；人工更正与结果版本一致；试听、摘要、导出、API 看到相同姓名。
- 性能：至少验证 60–120 分钟内部会议，记录峰值内存、积压、分离处理时长，设备切换与取消后可继续录音。

已验证：官网公开页面和操作文档；当前源码；六个模板 JSON 的解析与基础结构；Cargo workspace metadata 与两条配置警告。

未验证：完整前端/原生构建、目标机器录音、分离候选模型、中文准确率、文档渲染与跨平台运行。当前没有安装前端 node_modules，未找到 Bun；没有为本次评估安装依赖或下载模型。除了本评估文档，没有改动产品代码。

## 后续交付更新（2026-10-14）

用户已要求详细落地方案和归档清理。详细接口、迁移、五个独立实施计划见[内部增强版设计](superpowers/specs/2026-10-14-internal-enhancement-design.md)，执行记录见[归档清理](ARCHIVE_CLEANUP_2026-10-14.md)。上文“未改动产品代码”描述的是初次评估时点；后续已迁移测试样例、修正测试路径/说明并清理61个跟踪归档路径，增强功能仍未实现。
