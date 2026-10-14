# 内部增强版实施记录

目标：内部自用，对齐 Pro 核心功能，不做收费授权。执行顺序 P0 → P3 → P4，P1/P2 后置。本机验收环境见 [构建基线](ENHANCEMENT_BUILD_BASELINE.md)。

## 已完成并验证的任务

### P0 / F1：可复现构建基线

提交 `7ff4017`。锁定工具版本，统一本地/CI验证入口，修复被忽略的 Cargo profile/patch、OpenSSL4 原生编译和 Xcode27 发布宏库加载问题。原锁文件未变。现有前端45测试、原生245测试（3ignored）、类型检查、Next构建、CPU/Metal真实样例推理、.app生成与原生欢迎窗口均已验证。GitHub Actions、正式签名/公证不属于本次本机结果。

### P0 / F2：版本化会议文档

- 新增内容版本/canonical文档表及后续任务表；保留原十个迁移的原文和编号。
- 快照在同一读取事务内读取全量转写、标题、笔记、摘要和版本；251条样例包含最后一条，无时间/未知说话人保留null。
- 原秒时间显式转换毫秒；NaN、负数、溢出和逆序不伪造有效时间。
- 兼容旧摘要章节JSON、Markdown、BlockNote与仅存JSON的笔记；用户编辑器JSON独立保留，不把Markdown塞回JSON字段。
- 标题、转写、摘要、笔记写入按同事务递增版本；摘要/标题UI使用读取时的revision，成功后推进，冲突保留本地编辑并返回CONFLICT。
- 摘要返回内容与revision通过同一个SQL读取绑定；生成期间的来源冲突保护将在F3接入，本任务不声称已保护旧生成结果。
- 共享摘要校验拒绝无正文标题、空代码块和模型推理标记；快照不暴露带推理标记的历史摘要。
- Sidebar用native AppState就绪和database-initialized事件加载会议/模型；退役无前端调用的旧profile/FastAPI连接诊断及5167/8178 CSP。

F2提交 `cea601d`。验证：Rust253 pass / 0 fail / 3 ignored；前端50 pass / 0 fail，Node17 pass；类型检查、Next生产构建通过。Rust/TypeScript读取同一个公共JSON fixture。迁移回归先在原十个迁移后填旧数据，再升级，检查文本、时间、笔记JSON与外键删除行为。没有对用户历史会议运行删除或效果验收。

### P0 / F3：持久有界任务与摘要来源保护

- 任务与来源版本保存在 SQLite，单个重任务执行、最多16个排队任务；多连接 WAL 并发入队验证容量不会穿透。
- 摘要使用完整数据库快照，冻结转写/说话人、模型、模板、提示词与语言。密钥仅留内存，公共Job DTO不含参数和密钥。
- 排队期间来源变化会在调用provider前拒绝；运行期间变化则保留生成结果为stale，任务CONFLICT，不覆盖当前摘要/人工编辑。
- 结果、canonical摘要、摘要版本、生成标题与succeeded同事务提交；取消同步投影旧摘要状态，保留可读结果，晚到结果不能把已取消任务变成成功。
- 所有启动入口共享JobService；重启将queued/running标记interrupted，同时结束遗留pending摘要，必须显式重新提交。不会自动重发收费provider请求。
- 统一Job状态展示排队/执行/失败/取消/重启中断；排队时间不计入provider执行超时。保留旧摘要轮询与取消入口。
- 现有英文缓存指纹增加转写/说话人版本；生成自己的摘要/标题写入不会使同源语言切换缓存失效。

F3验证：Rust264 pass / 0 fail / 3 ignored；来源覆盖、取消晚到、恢复投影、冻结配置、模板路径校验与8连接并发测试通过。前端52 pass / 0 fail，Node17 pass，任务状态与长时间排队回归、类型检查、Next生产构建通过。未调用用户配置的收费provider。

### P0 / F4：恢复材料、受限积压与长摘要

DB/WAL/SHM备份、迁移前在线快照、有界采集与转写、持久补处理/显式恢复、停录队尾和最终状态保存、VAD连续30秒切块/静音释放、全provider预算及输出截断拒绝已实现。细节、故障注入与本机压力证据见 [验收记录](ENHANCEMENT_ACCEPTANCE.md)。

Rust290 pass / 0 fail / 3 ignored；前端55 pass / 0 fail，Node17 pass，类型检查、Next构建通过。120分钟等量PCM+真实tiny.en CPU压力：240段完整按序完成，pending0，92.535秒，RSS约270.4MiB。原生VAD依赖的两方法修正与出处随代码保留。

P0工程任务已落地；真实两小时设备采集、设备断开/磁盘满/强制退出的现场验收仍待验证，不能将合成压力等同真实会议效果通过。

## 接下来实施

- P4/D1–D5：本机模型/时间探针，真实时间映射、版本化分离结果和人工纠正。用户确认使用本机；带说话人标注的中文会议音频仍缺，不能据公共样例宣称DER或中文业务效果通过。
- P0真实设备现场验收仍待完成；P1模板管理与P2导出均未开始，继续按优先级最后处理。

### P3 / A1：本机授权网关基础

默认关闭，桌面显式启用后持久保存偏好；只绑定127.0.0.1随机端口。私有discovery仅pid/instance_id/base_url/protocol_version，重启刷新实例，关闭/退出清理自己的文件。拒绝恶意Host/Origin与第二个活动桌面实例。

256bit客户端token只显示一次，服务端SHA256与常量时间比较、每请求到期/撤销/scope检查；客户端token写入本机Keychain且读回验证后才提交DB。失败补偿清理、原生macOS后端已实测随机测试凭据的写入/读回/删除，未访问既有凭据。

限body1MiB、每token60次/分钟、并发请求4。设置页披露read覆盖当前用户全部会议，不含音频/模型密钥；首版仅创建read权限。A1此时只有health/capabilities，数据路由在A2接入。axum0.8.9/keyring3.6.3固定在锁文件；原十迁移不变。

验证：Rust296 pass / 0 fail / 3 ignored；6项原生网关测试、真实本机Keychain探针通过；前端新增一次性token展示/关闭与权限披露回归，类型检查、生产构建通过。此记录不代表A2–A5或P3整包完成。

### P3 / A2：有界只读 API

新增 `GET /meetings`、`/search`、`/meetings/:id/transcript`、`/meetings/:id/summary`、`/jobs/:id`。会议列表按 `created_at DESC, id ASC` 游标分页；转录按时间/时间戳/id 稳定分页，并在转录版本变化后拒绝旧游标。分页默认50、最大200；搜索要求1–256字符，将 `%`、`_` 与反斜线按字面匹配，片段最多300字符。摘要响应不返回完整转录/笔记，任务响应不暴露内部参数或结果正文。health capabilities增加 `meeting_read`、`jobs`，不声明导出能力。

验证：9项自动化 Rust 测试通过，覆盖251段转录跨页、删除会议后继续分页、版本游标冲突、重复/越界参数、敏感路径隔离及loopback读接口。`rustfmt` 针对改动文件通过；仓库全量 `cargo fmt --check` 报告大量既有文件格式差异，未对无关文件批量格式化。

### P3 / A3：CLI 与 MCP 只读客户端

新增独立 `automation-client` TypeScript 包，采用官方 MCP TypeScript Server SDK `@modelcontextprotocol/server` 2.3.1。CLI/MCP 都通过 app-data discovery 验证活动 loopback 实例，然后使用 macOS Keychain 中按 client ID 查找的凭据；一次性令牌不作为命令行参数。CLI 提供会议列表、搜索、转录分页、摘要与任务状态读取，stdout只输出JSON；MCP stdio注册相同五种只读工具，不提供shell、写入、录音或导出工具。新增 `docs/INTERNAL_AUTOMATION.md` 说明如何手工配置所选助手，不自动改动外部主机配置。

验证：包锁文件安装、TypeScript编译和5项 Bun 测试通过；包括陈旧实例、loopback 地址约束、CLI退出码/凭据隔离，以及MCP stdio仅列出五项只读工具的协议测试。新增 Secure MCP Tunnel 启动脚本，伪造 tunnel-client 的参数回归通过，runtime API key只通过环境变量传递且不进入argv。用户已选择 ChatGPT；官方 Secure MCP Tunnel 可通过本机出站客户端连接私有 stdio MCP，无需公网暴露。实际连接仍待 tunnel_id/runtime API key、Platform Tunnels Read + Use 权限及 ChatGPT 工作区权限；本轮未读取会议或既有Keychain项目。Pro MCP 仅提供 read/fetch，因此目前五个只读工具可接入，A4 写入/录音能力不能通过 Pro MCP 使用。

### P3 / A4：分权限幂等写入与录音控制

新增摘要任务、普通任务取消、录音开始/停止接口，分别强制write与record scope，并要求Idempotency-Key。幂等记录按client+key隔离，绑定规范化请求哈希，保存有界响应24小时；相同请求重放返回原结果，不同请求体返回CONFLICT，处理中重放返回BUSY。摘要写入携带expected_revision，旧版本在调用provider前拒绝。读凭据不可调用写/录音路由，设置页允许为单独客户端创建只读、写入或录音凭据。

桌面与API录音操作共享互斥状态；录音由应用统一生成session_id，停止时核对当前会话，旧session不能停止新会话。API不接收任意保存路径。幂等 action 完成但结果尚未持久化时，重试会返回BUSY以避免重复副作用；崩溃恢复/安全接管尚未实现，客户端需重新确认操作状态后使用新key。

验证：新增幂等重复结果/不同请求体冲突、旧录音session保护与读scope拒绝写/录音回归。原生Rust全量302通过、0失败、3忽略；前端57通过、类型检查、Next生产构建、MCP客户端构建及5项客户端测试通过。没有在此门禁中测试真实麦克风/系统音、设备断开或撤销进行中的客户端。

### P3 / A5：事务事件与签名 Webhook

新增 `recording.stopped`、`transcript.completed`、`summary.completed`、`summary.failed` 事件。转写/录音保存与摘要成功/失败终态会在同一SQLite事务追加事件并按订阅创建独立delivery；录音停止只在录音目录已保存、前端随后持久化会议/转写时发出，语义独立于转写完成。`diarization.completed` 在P4接入后再开放。

Webhook目标要求HTTPS和明确主机/IP许可。配置时验证DNS并固定解析到的地址，拦截私网/保留地址、禁止重定向；显式审批的精确内网IP可用于内部接收端。每个Webhook签名密钥随机生成、写入macOS Keychain并读回验证，数据库只存引用；创建后UI只显示一次。投递使用原始持久化JSON与 `timestamp + "." + raw_body` 的HMAC-SHA256，每次重试事件ID/正文稳定、时间戳/签名刷新。2xx成功，429/5xx/网络错误最多重试5次（10秒、1分钟、5分钟、30分钟、2小时），其他4xx终止；10秒超时，最多读取16KiB响应且不记录正文。关闭应用自动化会停止dispatcher；重启后恢复卡在delivering的delivery。停用单个订阅会暂停其待投递队列，重新启用后可继续。

验证：事件事务回滚、两个订阅各自入队、固定退避、状态码分类、签名原始正文、私网/metadata/link-local拦截、DNS地址变化、显式loopback IP审批和HTTP 302不跟随均有Rust测试。后续新增测试专用loopback HTTPS receiver，校验原始正文HMAC、503后重试成功、重复重试的事件ID/正文稳定、401终止及超时；最终DER证书fixture后Rust全量318通过、0失败、3忽略，定向格式检查通过。该测试使用显式测试根证书，不改变生产信任配置。真实助手宿主连接、真实macOS Keychain webhook密钥探针和真实接收平台验收仍待现场完成，故P3阶段门禁尚未全部通过。

### P4 / D1–D2：分离可行性门禁与转写时间

D1 数据准备已验证，但模型门禁未通过。`prepare_alimeeting_test.py` 从公开 AliMeeting Test 归档生成仓库外匿名集：20个会议、20份8通道远场录音、20份说话人标注TextGrid，最长轨道按会话合计646.591分钟；每个音频降混为单声道16 kHz，TextGrid转RTTM且说话人名匿名化。公开集与CC BY-SA 4.0归属说明保留在私有输出目录，归档、录音、RTTM和逐样本清单均不提交。

使用 sherpa-onnx 1.13.8 CPU API、pyannote segmentation int8、3D-Speaker ERes2Net embedding 对首个34.442分钟开发样本做诊断：无监督聚类阈值0.5/0.8/0.85/0.9/0.95/0.99分别为78.24%/51.38%/33.21%/32.56%/29.44%/28.68% DER，overlap DER为82.12%/56.94%/41.95%/40.96%/37.80%/36.81%；CPU RTF约0.07–0.08。随后冻结阈值并在剩余19场独立会议（612.149分钟）评测“预期人数已知”模式：人数直接取自RTTM参考标注，属于oracle人数上限，不是盲测或实际用户输入验证；微平均DER 37.94%、宏平均40.07%、仅5/19场≤20%，平均单场overlap DER 43.79%。按2/3/4人分组微平均DER分别为22.56%（8场，仍未达门槛）、56.10%（4场）、48.48%（7场）。性能平均RTF约0.067；准确率仍显著失败，故D1维持NO-GO，不进入D3–D5。完整模型训练集重叠审计、单人/中英切换样本、120分钟单文件资源压力、完整嵌入权重来源/再分发确认、取消和包体门禁仍未完成；下一轮应评估不同分离模型或分割/嵌入方案，而不是继续微调当前候选。

替代候选 MOSS-Transcribe-Diarize 0.9B（revision `704aa4a9c304e8520be88901e0d1960158ef5b15`）30秒开发片段 MPS/FP32耗时8.94秒、RTF 0.298，局部DER 0.51%、overlap DER 0.47%，4个说话人、11段。它是联合ASR/分离模型，且该样本不是独立holdout。完整34.442分钟开发会议在FP32下运行3分28秒时物理占用17.9 GiB、系统可用内存降至11%，主动停止；改为FP16后完整生成耗时2,534秒（42.24分钟，RTF 1.226），达到16,384 token上限，输出519段而参考有866段，截断输出DER 35.53%、overlap DER 36.91%，完成时MPS driver分配12.32 GiB。因为输出触及生成上限，该DER不代表完整转写质量；但不完整覆盖、慢于实时及高显存已经否定当前配置的上线可行性。MOSS不改变D1 NO-GO结论，也不进入D3–D5。

D2的时间映射、精度持久化及Whisper双解码保护已提交；末尾标点差异不再误判为文本缺失，英文单词边界仍需一致。录音、catch-up、导入、重转写路径都传递精确分段或保守降级。

追加验证：Rust全量326通过、0失败、4忽略；手工启用的模型测试以本机生成中文TTS和多语言tiny模型在CPU及Apple M5 Metal路径运行通过，两次解码保留同一尾句，只差末尾句号。JFK Metal烟测也通过。另以AliMeeting公开音频在CPU上运行首个120秒片段和8个30秒高语音占比片段，tiny模型9/9双解码测试完成，但canonical与带时间戳解码文本均不一致，安全返回`Unavailable`；同一30秒片段改用base模型也返回`Unavailable`。没有得到可评分的精确时间；符合回退合同，不算真实定位通过。模型把“会议纪要”误识，故不能据此声称识别质量；模型时间没有独立标注对照。之前标准Metal加载测试在受限沙箱内因`MTLCreateSystemDefaultDevice()`返回nil而SIGSEGV；沙箱外可见Apple M5并已完成Metal模型加载和推理。精确时间对照、双次解码真实性能和三入口试听仍待验收，D2及P4阶段均未完成。

新增公开TextGrid对照诊断：从AliMeeting Test `sample-01.wav`同一场会议选取10段无重叠标注语句，使用`ggml-base`双解码，3/10文本匹配并得到`Exact`，另7/10按合同返回`Unavailable`。3段`Exact`区间分别为0–2.88秒、0–3.58秒、0–4.96秒；TextGrid裁片内参考分别为0.30–2.77秒、0.30–3.38秒、0.30–4.88秒，边界误差依次为-300/+110、-300/+200、-300/+80毫秒，均未达到±250毫秒门槛。首段转写漏掉开头“那”并将“工作日”识别为“公会日”。因此`Exact`目前只说明两次解码文本匹配且区间合法，不能当作通过标注精度的证明。whisper-rs实验性token对齐在首段仅将终点改到2.81秒、起点仍为0，仍未达门槛，实验改动已撤回。新增忽略测试可通过音频路径、模型路径和裁片参考边界复现；样本、裁片、模型和输出仍在仓库外。十段均来自同一场会议，不构成跨会议准确率验证。D2保持未完成。

P4候选后续验证：在临时目录构建 MIT moss-transcribe.cpp（source `190a569c`，ggml `eced84c8`）并启用 Apple M5 Metal；CTest 5通过、0失败、9跳过（缺模型基线）。从 Apache-2.0 公共 GGUF 仓库下载 q8_0 权重并校验 SHA-256。30秒开发裁片生成11段/4人，DER 0.51%、overlap DER 0.47%。Metal进程耗时5.19秒（含加载）、峰值RSS 1.27 GB，但推理后在Metal residency-set清理处断言退出134；同裁片CPU四线程则干净退出，11.02秒（含加载，RTF 0.367），输出JSON与Metal逐字节相同。34.442分钟样本在37.73秒尝试分配45,659 MiB失败，峰值RSS 2.91 GB、退出139且无输出。该实现不能通过长录音或Metal稳定运行门禁；D1继续NO-GO，D3–D5不进入。

下一个候选为 NVIDIA Nemotron 3 Diarization：官方 NeMo-Speech.cpp v0.2.0 的 `metal-diar` 配置在 Apple Silicon 本机成功构建独立 diarization C API（CPU/Accelerate/Metal），产物和源码仅位于任务临时目录；没有模型权重或 Meetily 代码集成。模型 index 固定的 Q8_0 权重为107,012,128字节、SHA-256 `08456d9e...7c3a1`；模型卡声明最多8人、chunked长音频、10毫秒输出，并报告AliMeeting Test Far DER 10.47%（上游分数，非本机复现）。权重使用OpenMDW-1.1条款，用户确认是否接受前未下载或推理。详见 [D1 candidate note](DIARIZATION_SPIKE_RESULTS.md)。D1现仍为NO-GO。

补充筛选同一运行时支持的 Sortformer v2：模型最多4人、80毫秒输出步长，卡片提示主要面向英语并可能在非英语上退化；没有该v2在AliMeeting Test Far上的官方分数。该公开权重的模型卡声明下载即接受CC BY 4.0。arm64 `metal-diar` C ABI重复构建、分块输入/收尾/片段读取符号及Metal/CPU/BLAS链接检查通过，产物仅在任务临时目录；这是运行时/接口证据，不是模型推理或中文质量证据。用户对“仅本机公开AliMeeting评测、不打包发布”的CC BY授权仍待回复，权重未下载。D1仍NO-GO；D3–D5不进入。
