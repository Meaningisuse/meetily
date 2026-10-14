# 内部增强版验收证据

执行顺序 P0 → P3 → P4；P1/P2后置。区分代码/测试通过、本机真实运行、实际会议与业务效果验收。当前未使用用户的会议音频或收费 provider。

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
| 中文说话人效果 | AliMeeting Test公开标注集已在仓库外匿名准备；首样本无监督DER未通过 | P4保持NO-GO；未完成全留出集、120分钟资源、单人/中英切换验收 |

P0工程任务完成不意味着上述实际运行/业务验收已完成。P3、P4另按各任务验证并提交，不越过效果门禁宣称Pro对齐全部完成。

## 2026-10-14 本轮重新验证

- `scripts/check-enhancement.sh --all`（2026-10-14最新运行）：Node行为回归17通过；Bun前端57通过；TypeScript检查、Next静态生产构建、automation-client构建及5项客户端测试、Secure MCP Tunnel启动器隔离测试通过；原生helper构建；Rust库测试326通过、0失败、4忽略。Bun 1.3.14、CMake 3.31.6使用任务临时路径与`PYTHONPATH`，未修改全局安装。构建中有既存编译warning，但命令退出码为0。
- 最终DER fixture变更后，`scripts/check-enhancement.sh --native`：Rust库测试318通过、0失败、3忽略；定向 `rustfmt --check` 通过。
- `scripts/check-enhancement.sh --bundle`：release helper与Tauri macOS `.app` 打包成功，产物位于 `target/release/bundle/macos/meetily.app`。本次只验证构建/签名，不代表启动或实际设备录音；未配置Apple notarization凭证，因此跳过公证。
- Webhook集成测试新增Rust测试依赖并更新 `Cargo.lock`；没有改动pnpm锁文件。

P3代码与自动化测试已有提交。新增测试专用loopback HTTPS receiver端到端用例，以真实TLS请求验证固定目标IP、原始JSON正文HMAC、503重试后200成功、重复投递保持同一事件ID/正文、401终止以及服务端延迟导致的超时。ChatGPT Secure MCP Tunnel启动器已加入：运行测试以假的`tunnel-client`验证API key不进入argv，包测试统一包含该项；真实Platform/ChatGPT连接仍缺`tunnel_id`、runtime key及组织/工作区权限，真实macOS webhook Keychain读回和设备录音仍待现场验收。

P4 D1已将OpenSLR AliMeeting Test归档生成20个仓库外匿名样本，合计646.591分钟；数据和CC BY-SA 4.0归属说明留在私有目录。sherpa-onnx 1.13.8 CPU首个34.442分钟开发样本诊断：阈值0.5/0.8/0.85/0.9/0.95/0.99对应DER 78.24%/51.38%/33.21%/32.56%/29.44%/28.68%，重叠区DER 82.12%/56.94%/41.95%/40.96%/37.80%/36.81%。冻结配置后，在其余19场会议（612.149分钟）用RTTM参考标签提供预期人数，得到微平均DER 37.94%、宏平均40.07%、5/19场低于20%；该人数oracle模式仍远超质量门槛，不能代表盲测或真实用户提供人数的效果。2/3/4人组微平均DER分别22.56%/56.10%/48.48%，RTF平均0.067。故D1维持NO-GO，未进入D3–D5；完整模型训练集重叠审计、单人/中英切换样本、120分钟单文件资源、许可/打包和听音验收仍未通过。D2新增了受校验的Whisper分段时间传递：标准无时间戳解码仍是文本来源，第二次时间戳解码只有在文本一致且时间范围有效时才提供exact分段；失败时保留标准文本并标记时间unavailable。该路径已覆盖实时录音、catch-up、导入和重转写。公开AliMeeting的一个120秒片段与8个30秒高语音片段在tiny模型下9/9安全回退为Unavailable；base模型同一30秒窗口也回退，没有形成可评分的精确时间。多语言tiny模型加本机生成中文TTS的CPU及Apple M5 Metal双解码仍通过尾句保护，JFK Metal烟测通过；这些不证明真实会议时间精度。之前Rust测试进程SIGSEGV是受限沙箱中`MTLCreateSystemDefaultDevice()`返回nil造成；沙箱外Metal模型加载和推理通过。真实会议时间标注对照、双解码性能和三入口试听仍未验证，因此D2及P4均未完成。
