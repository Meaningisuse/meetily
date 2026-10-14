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

验证：包锁文件安装、TypeScript编译和5项 Bun 测试通过；包括陈旧实例、loopback 地址约束、CLI退出码/凭据隔离，以及MCP stdio仅列出五项只读工具的协议测试。统一前端门禁测试包括该包的锁文件安装、编译和测试。实际连接某个助手的主机级操作尚未验收，需用户选择目标助手；测试未读取会议或既有Keychain项目。

### P3 / A4：分权限幂等写入与录音控制

新增摘要任务、普通任务取消、录音开始/停止接口，分别强制write与record scope，并要求Idempotency-Key。幂等记录按client+key隔离，绑定规范化请求哈希，保存有界响应24小时；相同请求重放返回原结果，不同请求体返回CONFLICT，处理中重放返回BUSY。摘要写入携带expected_revision，旧版本在调用provider前拒绝。读凭据不可调用写/录音路由，设置页允许为单独客户端创建只读、写入或录音凭据。

桌面与API录音操作共享互斥状态；录音由应用统一生成session_id，停止时核对当前会话，旧session不能停止新会话。API不接收任意保存路径。幂等 action 完成但结果尚未持久化时，重试会返回BUSY以避免重复副作用；崩溃恢复/安全接管尚未实现，客户端需重新确认操作状态后使用新key。

验证：新增幂等重复结果/不同请求体冲突、旧录音session保护与读scope拒绝写/录音回归。原生Rust全量302通过、0失败、3忽略；前端57通过、类型检查、Next生产构建、MCP客户端构建及5项客户端测试通过。没有在此门禁中测试真实麦克风/系统音、设备断开或撤销进行中的客户端。

### P3 / A5：事务事件与签名 Webhook

新增 `recording.stopped`、`transcript.completed`、`summary.completed`、`summary.failed` 事件。转写/录音保存与摘要成功/失败终态会在同一SQLite事务追加事件并按订阅创建独立delivery；录音停止只在录音目录已保存、前端随后持久化会议/转写时发出，语义独立于转写完成。`diarization.completed` 在P4接入后再开放。

Webhook目标要求HTTPS和明确主机/IP许可。配置时验证DNS并固定解析到的地址，拦截私网/保留地址、禁止重定向；显式审批的精确内网IP可用于内部接收端。每个Webhook签名密钥随机生成、写入macOS Keychain并读回验证，数据库只存引用；创建后UI只显示一次。投递使用原始持久化JSON与 `timestamp + "." + raw_body` 的HMAC-SHA256，每次重试事件ID/正文稳定、时间戳/签名刷新。2xx成功，429/5xx/网络错误最多重试5次（10秒、1分钟、5分钟、30分钟、2小时），其他4xx终止；10秒超时，最多读取16KiB响应且不记录正文。关闭应用自动化会停止dispatcher；重启后恢复卡在delivering的delivery。停用单个订阅会暂停其待投递队列，重新启用后可继续。

验证：事件事务回滚、两个订阅各自入队、固定退避、状态码分类、签名原始正文、私网/metadata/link-local拦截、DNS地址变化、显式loopback IP审批和HTTP 302不跟随均有Rust测试。后续新增测试专用loopback HTTPS receiver，校验原始正文HMAC、503后重试成功、重复重试的事件ID/正文稳定、401终止及超时；最终DER证书fixture后Rust全量318通过、0失败、3忽略，定向格式检查通过。该测试使用显式测试根证书，不改变生产信任配置。真实助手宿主连接、真实macOS Keychain webhook密钥探针和真实接收平台验收仍待现场完成，故P3阶段门禁尚未全部通过。

### P4 / D1–D2：分离可行性门禁与转写时间

D1为NO-GO：没有独立获准的中文说话人留出集，模型栈权重分发许可未齐，质量/资源/离线打包门槛未测，故未进入D3–D5。D2的时间映射、精度持久化及Whisper双解码保护已提交；末尾标点差异不再误判为文本缺失，英文单词边界仍需一致。录音、catch-up、导入、重转写路径都传递精确分段或保守降级。

追加验证：Rust全量326通过、0失败、4忽略；一个需手工启用的模型测试以本机生成中文TTS及多语言tiny模型在CPU路径运行通过，两次解码保留同一尾句，只差末尾句号。模型把“会议纪要”误识，故不能据此声称识别质量；模型时间没有独立标注对照。Rust测试进程的标准Metal context加载在tiny和tiny.en模型上均于推理前SIGSEGV，具体原因未定，CPU测试不验证Metal路径。真实会议音频/RTTM、时间定位、双解码性能和三入口试听仍待验收，D2及P4阶段均未完成。
