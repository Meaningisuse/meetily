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
| 中文说话人效果 | 本机已确认，标注数据未提供 | P4效果门禁待验证 |

P0工程任务完成不意味着上述实际运行/业务验收已完成。P3、P4另按各任务验证并提交，不越过效果门禁宣称Pro对齐全部完成。

## 2026-10-14 本轮重新验证

- `scripts/check-enhancement.sh --all`：Node行为回归17通过；Bun前端57通过；TypeScript检查、Next静态生产构建、automation-client构建及其5项测试通过；原生helper构建；Rust库测试317通过、0失败、3忽略。
- `scripts/check-enhancement.sh --native`：Rust库测试317通过、0失败、3忽略。
- `scripts/check-enhancement.sh --bundle`：release helper与Tauri macOS `.app` 打包成功，产物位于 `target/release/bundle/macos/meetily.app`。本次只验证构建/签名，不代表启动或实际设备录音；未配置Apple notarization凭证，因此跳过公证。
- Bun 1.3.14与CMake 3.31.6使用任务临时路径，不修改全局安装；Cargo与pnpm锁文件未变。

P3代码与自动化测试已有提交；本轮完整回归通过，但阶段计划要求的实际助手宿主连接、HTTPS本地接收端投递及超时/500/401故障注入尚无验收证据。P4 D1为明确NO-GO；D2仅完成时间映射、精度字段及持久化合同，真实provider时间戳、中文尾句样本和三入口播放核对仍未完成。缺少授权的中文RTTM/转写样本、模型工件和许可证明，故不能把整个P4或Pro效果对齐标为完成。
