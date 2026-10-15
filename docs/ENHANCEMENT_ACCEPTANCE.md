# 内部增强版验收证据

执行顺序 P0 → P3 → P4；P1/P2后置。区分代码/测试通过、本机真实运行、实际会议与业务效果验收。当前未使用用户的会议音频或收费 provider。

## 2026-10-15 API 停录请求取消安全

修复范围：仅原生 HTTP 停录的请求生命周期。`idempotency::run_detached` 在首次数据库等待前建立独立 Tokio task，完整持有幂等登记、原生停录 action 与成功结果写入；20秒 HTTP deadline 和调用方取消只结束等待。不同 key 的停录 action 争用录音操作锁时立即返回 `BUSY`，不长期排队；获得锁后仍核对活动 session。响应格式、scope 和幂等重放合同保持。

验证证据：

- 红测：旧内联执行路径下，取消 waiter 后5秒内未保存结果，`detached_stop_finishes_and_replays_after_request_is_cancelled` 失败；Cargo退出101，0通过/1失败。修复后同一回归通过。
- 定向检查：`cargo test --locked -p meetily --lib automation -- --nocapture` 退出0，29通过/0失败。包含真实 loopback HTTP、生产鉴权及20秒 deadline；首请求返回原有 `BUSY` 后 action 仍完成并落库，同 key 重放得到相同成功结果且只执行一次。在途同 key 为 `BUSY`，不同请求体为 `CONFLICT`。
- 完整原生检查：使用仓库内CMake 3.31.6并为当前进程指定Xcode，`scripts/check-enhancement.sh --native` 退出0；Cargo metadata、helper准备完成，Rust库测试344通过/0失败/6忽略。忽略的模型测试未启用；已有编译warning仍在，不计作失败。
- 故障边界：取消 waiter 后正常业务错误仍清理登记；panic返回受控错误并保留未决 key；使用 SQLite trigger 注入成功结果写入失败，确认同 key 重试不重复副作用；原生录音锁争用、旧 session 防护和错误码映射有回归。
- 入口禁用：HTTP测试在超时后关闭 `enabled`，确认新请求被拒绝、已接收 action 仍落库。这不是完整 `AutomationServer::stop()` 或应用退出验收。

以上测试使用合成停录 action、测试数据库和内存 Vault，没有采集设备音频、加载模型或操作真实 Keychain。它们验证请求取消安全和幂等行为，不代表真实转写积压、文件保存与原生停录终态已完成现场验收；应用退出、runtime shutdown 和原生停录 panic 的业务恢复仍沿用既有边界。此修复不改变P3真实助手连接或P4说话人效果门禁。

## 2026-10-15 导入与重转写的 PCM 边界

新增共享 `BatchAudioLimits` 和音频批处理 lease：默认应用PCM工作预算2 GiB、scratch256 MiB、单packet16 MiB、FFmpeg临时WAV512 MiB。实际解码的rate/channels和累计capacity逐包受限；消费式原地mono/归一化、固定窗口串行sinc、VAD输出capacity检查及逐段移动替代全量复制。导入、重转写和metadata失败后的解码回退共享准入；阻塞任务及完成结果在释放PCM前持有lease。争用错误继续向前端发出既有错误事件。

- 红测：小预算WAV解码和预先取消两个回归在旧路径均收到成功结果，0通过/2失败，Cargo退出101；修复后通过。
- 定向音频检查：`cargo test --locked -p meetily --lib audio:: -- --nocapture` 退出0，144通过/0失败/4忽略；随后补充容量拒绝及已完成结果lease保持两项回归。
- 完整原生检查：`scripts/check-enhancement.sh --native` 退出0，358通过/0失败/6忽略（364项）；包含上述新增14项回归及之前停录20秒deadline回归。检查使用仓库CMake3.31.6和当前进程指定的Xcode；已有13条编译warning未影响退出码。模型/设备依赖测试保持忽略，未下载模型或启动真实录音。
- 覆盖实际WAV超限、packet间取消、capacity扩容前拒绝、上采样及整数边界、mono移动、串行sinc抗混叠与取消、VAD输出预算、片段时间保留、真实FFmpeg触限文件拒绝与临时清理、合成子进程回收和共享lease生命周期。

这是应用层分配规则及取消验证，没有实测整个应用RSS或长会议识别质量。120分钟16 kHz mono仅通过默认预算算术检查，不是两小时导入验收。重转写预算失败仍在旧转写替换事务之前返回，这一结论由控制流复核支持，没有新增真实旧库故障注入测试。完整默认上限、源格式对可接受长度的影响、FFmpeg有限超调及取消粒度见[音频批处理边界](BATCH_AUDIO_LIMITS.md)。

## 2026-10-15 统一资源调度与录音优先

AppState.jobs持有实例级ResourceScheduler，录音、本地摘要、导入、重转写、元数据解码回退和保存音频补转写共享准入。录音意图原子阻止新本地重任务；已有任务自然排空后才启动采集，等待上限15秒，超时撤销意图并返回BUSY。本地摘要保持queued，批处理和恢复即时拒绝；固定云服务摘要保留原串行队列但不占本地许可。两条录音入口均已接入，许可覆盖停止worker排空、引擎卸载和保存收尾。

本地摘要取消后会排空当前HTTP响应再归还资源，完整响应与业务结果分别记录；参数／length／JSON／完整HTTP错误不误封禁。传输中断、任务abort、panic及未确认退出的helper保留uncertain。内置helper退出要求wait或kill后wait确认。录音stream／pipeline清理错误向上传播并继续保存材料；manager启动后的失败不能统一确认rollback，因此保守阻断，需确认本地工作已停止并重启。详见[调度政策与失败边界](RESOURCE_SCHEDULING.md)。

- 红测：新增3项录音优先状态机回归在stub路径全部失败（0通过/3失败，Cargo退出101），完成实现后通过。另补pipeline handle注入panic的红测，0通过/1失败（退出101），确认底层JoinErr曾被吞为成功，修复后再次全量通过。
- 最终原生检查：`scripts/check-enhancement.sh --native` 退出0；Rust共383项，377通过、0失败、6忽略，比本项前增加19项回归。使用仓库CMake3.31.6、进程级Xcode覆盖；helper构建及FFmpeg缓存校验成功。helper1条、build script1条、lib test11条warning未影响退出码。
- 覆盖原子并发准入、录音等待防插队、超时／放弃等待、异常drop／panic、queued摘要取消与来源冲突、固定云摘要与录音许可并行、本地类别别名、batch阻塞工作及完成结果持有许可。合成HTTP验证Ollama／自定义服务取消后body仍阻塞时不放行、排空后放行、断连／abort保守阻断及业务错误正常释放。清理注入覆盖stream／pipeline失败和成功，真实pipeline handle panic回归确认JoinErr向上传播；合成子进程验证helper确认退出后才能释放资源。
- 前端：现成Bun1.3.14执行`bun test tests`（frontend目录），59通过、0失败；`./node_modules/.bin/tsc --noEmit`退出0。等待提示使用recording-starting的message，自动化start保留BUSY错误类别。前端测试存在预期的模拟失败分支日志，不代表测试失败。
- Astra只读方案与修复复核实际参与；Luna执行红测和最终验证，主代理完成实现及测试设计。首次全量编译发现2处String借用错误，修复后重跑通过；复核指出的清理错误吞没及摘要业务错误误封禁已修正后再次执行最终原生检查。

验证使用独立调度器实例、测试数据库、合成HTTP及子进程，没有启动真实录音、加载真实模型、操作用户会议或真实Keychain。没有执行新.app打包／启动验收、真实设备rollback故障、长会议资源峰值或外部服务取消协议验收。调度约束本应用发起的任务，不保证整机资源独占；内置helper每次生成后退出可能增加分块摘要的模型重载时间。

## 2026-10-15 Whisper日常单次解码

`transcribe_audio_with_verified_timing`保留现有调用接口，日常路径只执行一次原有的无时间戳canonical解码，直接消费输入PCM，不再复制样本或隐式运行第二次完整推理。文本、confidence、is_partial及解码错误沿用canonical结果。provider层返回空候选时间与Unavailable；录音、catch-up、导入和重转写仍由原有调用方保留整段chunk/VAD的approximate区间，不生成未经验证的细粒度时间。

原双解码逻辑移至显式Rust诊断入口`transcribe_audio_with_timing_diagnostics`，没有新增用户设置或默认环境开关。既有两个忽略的模型诊断测试改为调用此入口；文本匹配且区间合法仍不能宣称Exact，候选子段继续受原精度门禁抑制。日常四个调用点的接入由源码核对，未在本轮逐一启动真实应用流程。

- 红测：先等价提取旧双解码编排，在相同生产helper上注入native解码边界。2项回归均按预期失败（退出101、0通过/2失败）：实际调用false与true两次，且canonical收到的PCM地址与原缓冲不同。
- 定向检查：`cargo test --locked -p meetily --lib whisper_engine::whisper_engine::tests::`退出0，18通过、0失败、2忽略。新增5项非模型回归覆盖一次解码、PCM移动、中文尾句和canonical返回字段、错误传播、未加载模型错误，以及诊断匹配／文本不一致／候选失败时的精度门禁。
- 完整原生检查：`scripts/check-enhancement.sh --native`退出0，共388项，382通过、0失败、6忽略。使用仓库CMake3.31.6与进程级Xcode覆盖；helper构建和FFmpeg缓存校验成功。helper1条、build script1条、lib test11条warning未影响退出码。
- 主代理完成根因核对、实现和测试设计，Luna实际执行红测、定向和全量原生验证。没有修改前端或IPC参数，因此本项没有重复前端检查。

测试替换了耗时的native推理边界以验证实际生产编排，没有加载真实模型、录制音频或使用用户会议。确认的是日常解码调用由两次变为一次，以及移除该层PCM复制；没有测量端到端耗时、GPU负载或峰值RSS，不能宣称总耗时减半。没有重新打包／启动.app。D2独立时间精度和三入口试听仍待验收，P4门禁不变；未知／近似时间的前端显示属于后续独立修复项。

## 2026-10-15 未知／近似时间显示与跳播

分页、录音页和会议详情的非分页转换现统一使用`transcriptsToSegments`，保留缺失／null的audio_start_time、audio_end_time和timing_precision，不将未知开始时间补为0。共享显示策略`getTranscriptTiming`同时决定标签、读屏说明和跳播目标；小列表与虚拟列表都完整传递起止时间及精度字段。

- 缺失、null、负数、非有限开始时间、无效／倒置结束时间，以及明确unavailable的时间显示`[--:--]`，无跳播按钮。结束时间或墙上时钟字符串不会被用来猜测录音起点。
- approximate显示`[约 00:12]`；有有效开始时间但没有精度字段的历史记录也保守标为近似。悬浮和读屏说明披露近似语义，点击传递原始秒数，例如12.5，不使用已格式化的整数秒。
- 真实0秒仍可跳播；exact显示`[00:00]`，approximate或无精度字段显示`[约 00:00]`。没有播放回调时所有时间只读。
- TooltipTrigger改为asChild，直接附着时间按钮或span；未知时间不会被Tooltip隐式包成按钮，也不会生成嵌套按钮。本项只修改前端投影和交互，不改写历史转写数据或提升后端时间精度。

验证由Luna执行，主代理核对实现、日志和提交差异：

- 红测：2个定向文件共18项，11通过／7失败，退出1。实际复现未知时间仍可点击、近似／历史时间未标注，以及缺失和null开始时间被补成0。修复后同组18项全部通过。
- 最终完整前端测试：现成Bun1.3.14执行`bun test tests`，71通过／0失败，16个文件、269次expect；相比本项前新增12项回归。覆盖分页转换、小列表／虚拟列表、非法起止值、明确unavailable、真正0秒、原始小数秒跳播、历史精度降级和超过60分钟的格式。虚拟列表测试仅替换DOM测量，保留实际时间渲染和点击处理。
- `./node_modules/.bin/tsc --noEmit`退出0；`pnpm build`退出0，完成生产编译、类型检查、11页静态生成与构建追踪。最终复跑显式使用项目要求的Node24.14.0和pnpm10.32.1；早期Node26构建不作为最终版本证据。未安装或升级依赖。
- 独立Bun进程无mock进行React SSR标记核对：真实VirtualizedTranscriptView与TooltipProvider中，未知／null／unavailable三段均无button；exact0和approximate12.5恰好生成两个按钮，未出现Tooltip额外／嵌套按钮。退出0。
- `git diff --check`通过。本项没有修改Rust或IPC命令参数，未重复原生测试。

没有启动Tauri应用、读取真实会议或实际播放录音。SSR标记与React交互测试不代替浏览器键盘／读屏工具、真实音频跳播偏差、长列表实际滚动或三入口现场试听验收；D2时间精度和P4效果门禁保持原边界。

## 2026-10-15 摘要完成事件的最终版本一致性

`summary.completed`现在在摘要正文、任务结果／成功状态及生成标题全部写入后，通过同一事务读取最终全局revision，再追加事件和投递行。保留摘要与标题原有的独立版本递增语义；标题变化时最终版本为N+2，不变化时为N+1。事件表的revision与持久化JSON都描述该次完成事务，避免消费者在没有其他编辑时使用事件版本就发生条件写入冲突。

- 红测：真实SQLite测试新增标题变化场景，事件实际revision1、最终文档2，断言失败；Cargo退出101、0通过／1失败。
- 表驱动覆盖标题变化、相同、缺失、空白、超长、非字符串6种输入，检查最终文档／事件列／JSON、summary_revision只递增一次、job成功。重复完成不新增event／result／delivery，事件版本可用于下一次条件写入；后续人工编辑不重写旧事件版本。
- 强化来源冲突和取消的既有回归：晚到生成标题不覆盖当前标题，也不发出completed。强化append失败回滚：有效订阅先生成delivery，之后损坏filter触发错误，摘要、标题、版本、canonical、任务结果／状态、event和delivery全部回滚；修复filter后重试成功。
- 定向Rust：摘要仓库4通过／0失败，jobs13通过／0失败。完整`scripts/check-enhancement.sh --native`退出0，共389项，383通过、0失败、6忽略。使用既有CMake及进程级Xcode覆盖，无依赖安装；模型／设备相关忽略项保持忽略。
- Astra完成只读事务方案及代码复核，主代理实现，Luna执行红测和GREEN；没有修改schema、事件协议、签名和投递重试，也不修写历史已保存的事件。并行更新通道的文件归属独立，按另一项单独提交。

验证使用测试数据库和本地outbox行，没有访问真实会议、Keychain或Webhook接收器。事件版本只保证对应其完成事务；若投递前已有后续编辑，接收端仍必须正常处理版本冲突。未对生产历史事件做迁移，也未声称完成真实助手／Webhook业务验收。协议说明同步见[内部自动化](INTERNAL_AUTOMATION.md#webhook-接收)。

## 2026-10-15 内部增强版更新通道保护

内部增强版现统一使用手动安装内部包的更新策略。普通／强制检查只读取当前安装版本，明确返回`manual-install-required`，不把没有可用更新解释为“已是最新版”。关于页面和托盘入口展示内部安装说明；版本读取失败显示“版本未知”，说明仍可打开。旧更新元数据不会重新产生下载按钮或通知，持有旧Update对象直接调用安装服务也会在副作用前拒绝。

原生启动代码移除updater插件注册，Tauri配置移除社区更新源、公钥和updater权限，关闭updater发布产物。应用标识、现有版本号、会议数据目录、依赖与锁文件保持原状；未确认独立内部更新源和签名体系前，不提供虚构配置。策略及未来启用条件见[内部增强版更新策略](INTERNAL_UPDATE_POLICY.md)。

- 红测：原始8项回归全部按业务断言失败（退出1、0通过／8失败）；随后追加版本读取失败回归，复现界面永久显示“版本读取中”（退出1、0通过／1失败）。两轮均未访问真实更新源。
- 最终定向：`bun test tests/lib/internal-update-policy.test.tsx`退出0，9通过／0失败、44次expect。覆盖普通／强制检查、旧对象安装拒绝、启动无计时器、过期更新元数据、静默通知、关于页面正常和失败分支、托盘事件以及实际Tauri配置解析。
- 最终前端全量：`bun test tests`退出0，80通过／0失败，17个文件、313次expect。`tsc --noEmit`退出0且无诊断；`pnpm build`退出0，完成Next生产编译、类型检查和11页静态生成。使用Bun1.3.14、Node24.14.0及pnpm10.32.1，未安装或升级依赖。
- 最终完整原生：`scripts/check-enhancement.sh --native`退出0，389项中383通过、0失败、6忽略，21.33秒。使用既有CMake与进程级Xcode覆盖；helper1条、build script1条、lib test11条warning不影响退出结果。模型／AliMeeting时间相关忽略项未执行。
- Sol实现代理独立负责更新模块，主代理负责并行摘要事务修复、测试设计和最终差异核对；Astra完成只读复核，发现的版本失败态和托盘标签问题已修正；Luna执行红测及最终全部检查。两个问题分别提交。

前端测试替换原生／联网边界，原生检查完成编译及库测试；本项未重新打包、安装或启动.app，未执行真实用户升级。仍引用`.sig`／`latest.json`的CI发布流程未改造或验收，因此不能宣称内部安装包发布、签名、回滚及实际升级已完成。没有读取真实会议、音频、Keychain或加载真实模型。

## 2026-10-15 API 开始录音请求取消安全

开始录音HTTP入口现通过`start_recording_with_action`与已有`run_detached`持有完整幂等登记、原生初始化和成功响应持久化，20秒HTTP超时或waiter取消不再丢弃已接收操作。API原生包装使用`try_lock`即时拒绝录音操作锁竞争，直接调用既有inner，并在持锁期间读取本次session；不会在长停录后排队迟到启动。保留原请求字段校验、scope、请求哈希、15秒资源等待与成功响应结构。桌面公开开始包装和停录实现保持原合同。

- 红测：API锁竞争回归在旧等待实现下50毫秒未返回，退出101、0通过／1失败；生产guard的真实20秒deadline后仍无成功结果，25.02秒时断言失败，退出101、0通过／1失败；取消waiter后5秒未完成结果／错误清理，退出101、0通过／1失败。三项均编译成功并按预期业务断言失败。
- 修复后三项定向分别1通过／0失败，退出0；HTTP用例实际20.03秒。成功后原key重放同session且只执行一次，进行中同key为BUSY；关闭入口阻止新请求但已接收操作仍完成。abort用例覆盖成功持久化、普通BUSY错误清理及重新提交、不同body同key冲突。
- 最终`scripts/check-enhancement.sh --native`退出0，391项中385通过、0失败、6忽略，22.00秒。使用既有CMake及进程级Xcode覆盖；helper1条、build script1条、lib test11条warning未影响退出码。未安装依赖，未改全局环境。
- Astra完成只读方案与代码复核，主代理实现及设计回归，Luna执行RED、定向GREEN和完整原生检查。测试共用生产HTTP guard及action helper，以合成action代替设备初始化；字段校验与锁内读取session由源代码结构复核，不将其写成已完成设备验收。

超时后录音仍可能开始，客户端应使用原key及相同请求体重试取得session，不能当作未执行而自动换key。文档同步见[原生HTTP开始录音](INTERNAL_AUTOMATION.md#原生-http-开始录音与超时重试)。测试未启动设备、加载模型、访问真实会议或Keychain，也没有打包／启动.app；panic、结果写入失败及应用退出后恢复仍沿用保守边界。本项不宣称所有写API均已取消安全：摘要创建与普通任务取消的内联执行缺口仍需独立修复。

## P0 / F4 工程验证

- 数据库恢复：异常打开前保留 DB/WAL/SHM；SHA256清单、私有目录、fsync与发布后读回。备份失败不会继续破坏原始文件。正常升级用 SQLite online snapshot，测试包含未checkpoint的已提交WAL和迁移前旧数据。没有自动删除原WAL或声称能修复损坏的数据库。依据：[SQLite VACUUM INTO](https://www.sqlite.org/lang_vacuum.html)。
- 音频积压：采集callback用有界try_send，满时报告不可恢复错误并停止；原始队列256、保存64、转写8、worker1。保存会排空队尾，最终落盘失败返回错误。原始音频检查点保留到音频/最终转写/metadata均保存成功；incomplete/error保留材料。
- 转写补处理：开启保存时，每个送provider的PCM先写入私有.catchup，1GiB/20,000段上限，单段16MiB；校验与损坏检测，成功结果先持久化再去pending。恢复读取结果总量16MiB上限。满队列后依序留盘，停录排空；重启通过现有恢复对话框显式重放未完成段。失败不标记保存，不删除检查点。模型返回partial、报错或停录预算到期均保留incomplete状态。
- 不保存音频：满队列明确停止且说明没有恢复音频；仍保存最终转写和状态，保留停录后完成的最后一句。文件通过唯一临时文件、0600、fsync、rename写入。关闭保存不等于可恢复音频。
- VAD：旧实现内部保留全部静音、无限发言直到停顿。已用30秒窗口按样本连续输出，静音trim，最后尾段不丢。发现锁定silero依赖trim候选起点和take_until结束起点缺陷，vendor原MIT依赖修正两方法；详情见vendor/silero/MEETILY_PATCH.md。原时间/批处理回归覆盖。
- 长摘要：UTF8字节保守预算，保留序列与完整尾段；实际模板、系统、包装、输出预算统一计入，所有provider走相同切块/逐级合并。最多8层且必须收敛，超预算/截断显式失败。Ollama使用原生/api/chat的num_ctx/num_predict，窗口最多32768；不把模型理论context当成实际运行窗口。依据：[Ollama chat](https://docs.ollama.com/api/chat)、[context length](https://docs.ollama.com/context-length)。BYOK未知模型采用4096假设，小于该窗口的模型仍可能拒绝请求，大窗口会增加分块；不宣称精确tokenizer计数或语义完整性。
- 截断：OpenAI兼容finish_reason=length、Claude stop_reason=max_tokens、Ollama done=false/length不会被当作成功摘要。内置helper未提供同等finish_reason，不能据字数推断完整性。

## 本机120分钟等量压力

环境：Apple M5 /24GiB，macOS27.0.1。命令：

```bash
cargo run --locked -p meetily --example processing_stress -- /path/to/ggml-tiny.en.bin
```

公开JFK音频重复成240段实际PCM，每段30秒，总7,200秒。先加速送入8槽队列并积压240段，再逐段用真实tiny.en CPU推理；源PCM/输出仅存临时目录，程序结束删除自己的测试目录。不采集设备，不写用户会议库。

结果：240段按0–239顺序完成，最后结束时间7,200秒，pending=0；92.535秒实际用时；本进程采样峰值RSS276,912KiB（约270.4MiB）。该RSS来自/bin/ps，不是连续高频采样或全应用峰值；该测试不包含麦克风/系统采集、混音、FFmpeg连续保存、前端或真实中文识别，也不等同真实运行120分钟。

## 仍待本机现场验收

| 项目 | 当前证据 | 结论 |
|---|---|---|
| 两小时真实麦克风+系统音录音 | 合成等量队列压力、VAD回归、保存排空测试 | 待现场验收 |
| 真实设备断开/权限撤销 | 现有设备回归与错误停止逻辑 | 待现场验收 |
| 真实磁盘满 | 已接受输入保留、预算满/坏存储目标注入测试 | 待现场验收 |
| 强制退出后真实会议恢复 | 重启pending/校验/去重/恢复UI测试 | 待现场验收 |
| 中文长会议摘要质量 | 中文预算及尾行动项mock HTTP回归 | 待真实会议验收 |
| 中文说话人效果 | Sortformer v2本机跑完19场/612.149分钟，主DER微平均4.03%、宏平均4.37%；overlap DER 7.89%/7.52%，19/19低于20%；C ABI CPU流式组件完成138.018分钟单流压力测试，峰值RSS 421 MiB | 固定模型卡声明`Alimeeting-train`；但同revision `.nemo`配置的train/validation/test manifest路径均为`null`，公开模板未提供实际session清单。没有发现Eval/Test用于此权重的证据，也未能核实完整训练谱系，因此不能认定为完成session审计的独立holdout或已知泄漏；同一34.442分钟session重复4次不是独立质量或多session证据；整文件Metal CLI峰值RSS 2.82 GiB仍超门槛，完整Meetily链路和Metal流式路径未验证；P4整体保持NO-GO，尚缺独立session排除、单人/中英切换和真实会议验收 |
| 转写时间戳试听入口 | 已接通 meeting ID 到会议目录内唯一音频文件的安全解析、原生流式播放与时间戳跳播；Rust边界测试3项、前端行为测试2项、类型检查通过 | 已实现录音/导入/重转写共用UI链路，尚未在应用中对同一公开样本实际完成三入口播放和跳播验收 |

P0工程任务完成不意味着上述实际运行/业务验收已完成。P3、P4另按各任务验证并提交，不越过效果门禁宣称Pro对齐全部完成。

## 2026-10-14 本轮重新验证

- `scripts/check-enhancement.sh --all`（Parakeet近似时间接入及文本一致性保护后复核）：Node行为回归17通过；Bun前端57通过；TypeScript检查、Next静态生产构建、automation-client构建及5项客户端测试、Secure MCP Tunnel启动器隔离测试通过；原生helper构建；Rust库测试331通过、0失败、6忽略。Bun 1.3.14、CMake 3.31.6使用任务临时路径与`PYTHONPATH`，未修改全局安装。构建中有既存编译warning，但命令退出码为0。另手动运行被忽略的Parakeet模型烟测，使用仓库内JFK fixture和仓库外固定权重，结果为22个有界`approximate`词段且与canonical文本一致。
- 最终DER fixture变更后，`scripts/check-enhancement.sh --native`：Rust库测试318通过、0失败、3忽略；定向 `rustfmt --check` 通过。
- Parakeet转换单测4项通过，覆盖canonical文本保留、非法/越界时间回退及零长度词段防护；录音保存精度传播测试通过。完整Rust计数见本轮`--all`结果。
- `scripts/check-enhancement.sh --bundle`：release helper与Tauri macOS `.app` 打包成功，产物位于 `target/release/bundle/macos/meetily.app`。本次只验证构建/签名，不代表启动或实际设备录音；未配置Apple notarization凭证，因此跳过公证。
- Webhook集成测试新增Rust测试依赖并更新 `Cargo.lock`；没有改动pnpm锁文件。

当前 HEAD `a0e8f14` 的复核：重新运行 `scripts/check-enhancement.sh --all`，Node 17项、前端57项、Automation客户端5项及Rust 331项全部通过（Rust 0失败/6忽略）；再运行 `scripts/check-enhancement.sh --bundle` 成功生成 macOS `.app`。`codesign --verify --deep --strict` 确认签名有效；应用可执行文件 SHA-256 为 `c539f0894a331b7aa4c37ef9b33c1eb2fbdfdbceb6b89274bf2715a79fdebdbe`。因未配置Apple公证凭证，未公证；本次未启动录音或触碰用户会议数据。构建存在已有编译warning，未影响退出结果。

P3代码与自动化测试已有提交。新增测试专用loopback HTTPS receiver端到端用例，以真实TLS请求验证固定目标IP、原始JSON正文HMAC、503重试后200成功、重复投递保持同一事件ID/正文、401终止以及服务端延迟导致的超时。ChatGPT Secure MCP Tunnel启动器已加入：运行测试以假的`tunnel-client`验证API key不进入argv，包测试统一包含该项；真实Platform/ChatGPT连接仍缺`tunnel_id`、runtime key及组织/工作区权限，真实macOS webhook Keychain读回和设备录音仍待现场验收。

P4 D1已将OpenSLR AliMeeting Test归档生成20个仓库外匿名样本，合计646.591分钟；数据和CC BY-SA 4.0归属说明留在私有目录。sherpa-onnx 1.13.8 CPU首个34.442分钟开发样本诊断：阈值0.5/0.8/0.85/0.9/0.95/0.99对应DER 78.24%/51.38%/33.21%/32.56%/29.44%/28.68%，重叠区DER 82.12%/56.94%/41.95%/40.96%/37.80%/36.81%。冻结配置后，在其余19场会议（612.149分钟）用RTTM参考标签提供预期人数，得到微平均DER 37.94%、宏平均40.07%、5/19场低于20%；该人数oracle模式仍远超质量门槛，不能代表盲测或真实用户提供人数的效果。2/3/4人组微平均DER分别22.56%/56.10%/48.48%，RTF平均0.067。替代候选MOSS-Transcribe-Diarize 0.9B在同一开发会议30秒裁片得出局部DER 0.51%、overlap DER 0.47%（MPS FP32 RTF 0.298），但它是联合ASR/分离模型且非独立holdout。完整34.442分钟FP32运行曾在3分28秒观测到17.9 GiB物理占用、系统可用内存11%，后主动停止；FP16重测完成但42.24分钟（RTF 1.226），达到16,384 token上限，输出519段（参考866段），截断输出DER 35.53%、overlap DER 36.91%，MPS driver分配12.32 GiB。该截断DER不能作为完整输出质量分数，但输出不完整且慢于实时已不满足当前本机门槛。D1仍为NO-GO，不进入D3–D5；模型训练集会话重叠未核实，缺单人/中英切换样本；Sortformer虽完成138分钟合成单文件压力测试，但RSS超2 GiB，未通过内存门槛。自然会议长时、包内运行、分发许可和听音验收仍未通过。D2已覆盖实时录音、catch-up、导入和重转写的原录音时间映射，并保留标准无时间戳解码作为文本来源。由于公开标注对照发现Whisper双解码的3/10个结构合法候选超出±250ms，现将候选子段时间降级隐藏，不再标为exact；当前保留VAD/chunk approximate范围，直到独立强制对齐通过精度验收。公开AliMeeting的一个120秒片段与8个30秒高语音片段在tiny模型下9/9安全回退为Unavailable；base模型同一30秒窗口也回退，没有形成可评分的精确时间。多语言tiny模型加本机生成中文TTS的CPU及Apple M5 Metal双解码仍通过尾句保护，JFK Metal烟测通过；这些不证明真实会议时间精度。之前Rust测试进程SIGSEGV是受限沙箱中`MTLCreateSystemDefaultDevice()`返回nil造成；沙箱外Metal模型加载和推理通过。真实会议时间标注对照、双解码性能和三入口试听仍未验证，因此D2及P4均未完成。

替代的 ggml/C++ MOSS q8_0 候选也已在 M5 上做隔离评估：Metal 构建完成，CTest 5通过、9跳过；30秒开发裁片 DER 0.51%、overlap DER 0.47%，CPU四线程11.02秒干净退出，输出与Metal逐字节相同，但Metal进程在清理时以134退出。完整34.442分钟录音需分配45,659 MiB，37.73秒后以139退出且无输出。故其长录音和Metal稳定运行门禁失败，不改变D1 NO-GO结论。

D2时间精度补充验收：在AliMeeting Test同一会议的10段无重叠标注语句中，`ggml-base`双解码3/10返回`Exact`，7/10安全返回`Unavailable`。3个`Exact`的起点均早300毫秒；结束边界误差为+110、+200、+80毫秒，均未达到±250毫秒精度门槛；首段ASR还漏词并误识一个词。实验性Whisper token对齐仍未达门槛并已撤回。该结果证明此前`Exact`仅代表解码一致/范围合法，并未满足时间精度验收。代码现以回归测试确保未验证候选不再导出为exact子段；首个裁片复跑后返回`Approximate`且不导出候选时间。详细数据与复现方式见`DIARIZATION_SPIKE_RESULTS.md`。独立强制对齐、三入口试听仍待验收，D2及P4尚未完成。

新的D1候选可行性检查：NVIDIA NeMo-Speech.cpp v0.2.0 在 Apple Silicon 本机成功构建 standalone diarization C API 的 Metal preset。Nemotron 3 Diarization Q8_0 artifact 的模型许可为 OpenMDW-1.1；尚未下载权重，也没有执行模型推理。官方模型卡的 AliMeeting Test Far DER 10.47% 仅作候选资料，不能替代本机评测。条款授权待用户回复；D1仍NO-GO，D3–D5保持暂停。

D2映射安全补充：为统一实时录音、catch-up、导入、重转写的 provider 局部时间映射，新增长度/顺序/文本/溢出校验；任一分段越界或偏移溢出时整组回退到 canonical 文本和整段 approximate 范围。回归覆盖非零源偏移、跨词序列映射、越界与整数溢出；`cargo test --locked -p meetily --lib` 为333通过、0失败、6忽略。修改后的 release `.app` 打包成功，严格签名校验通过，主程序 SHA-256 `01d7247c811e4962d75203ef1a72945b4f112880ade4bb010fc020a1263cc032`；未公证，也未启动录音。D2仍缺独立标注精度通过和三入口试听。
