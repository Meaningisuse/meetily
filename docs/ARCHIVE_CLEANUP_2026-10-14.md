# 归档代码清理记录

执行日期：2026-10-14。用户授权：“输出详细的落地改造方案，清理归档代码”。基线：Community 0.4.1，完整 commit 见 [机器清单](ARCHIVE_CLEANUP_MANIFEST_2026-10-14.json)。范围是退出当前工作树的归档代码；不是实现增强功能，也不是删除活动兼容数据或重写当前架构。

## 1. 已清理内容

共移除 **61 个 Git 跟踪路径**（含 1 个子模块 gitlink），另修改 4 个现有文件，添加方案文档和独立测试样例。跟踪文件 diff 约删除 3.08 万行；这个数字不计子模块内部源码。清理执行时未暂存或提交；后续提交状态以Git历史为准。未推送。

| 内容 | 跟踪路径数 | 清理依据 |
|---|---:|---|
| `backend/` | 41 | CLAUDE.md已定义为归档且不支持：Python/FastAPI、Docker、独立Whisper服务器、旧开发脚本、环境样例与子模块；当前Cargo/前端构建无此依赖 |
| `.gitmodules` | 1 | 仅声明已删除的 `backend/whisper.cpp`；没有其他子模块 |
| `frontend/src-tauri/src/audio_v2/` | 9 | lib.rs不声明此模块；未接入的第二套音频原型 |
| `audio/core-old.rs`、`recording_commands.rs.backup`、`recording_saver_old.rs` | 3 | 活动模块使用新分拆代码/同名正式实现，备份文件不参与编译 |
| `audio/stt.rs` | 1 | audio/mod.rs未声明，引用当前不存在的pyannote/screenpipe模块；不代表已有分离能力 |
| `src/lib_old_complex.rs` | 1 | 非Cargo入口，替代为现有lib.rs和分拆模块 |
| `frontend/build_backup.bat` | 1 | 未引用的旧Windows构建脚本备份；当前build/clean_build脚本保留 |
| `src-tauri/config/backend_config.json` | 1 | 未引用的历史5167/8178服务配置 |
| `frontend/API.md`、`src-tauri/CLEANUP_PLAN.md` | 2 | 历史独立服务器API说明和未执行的旧清理建议；本记录替代 |
| `frontend/vs_buildtools.exe` | 1 | 4.46MB未被脚本引用的安装器，当前README指导从微软官方安装开发工具 |

全部路径逐条列在机器清单中。父仓库 `.git` 历史、子模块对象库和配置未修改，磁盘上的Git历史仍会占空间；这次不是Git历史瘦身。

## 2. 为保持当前测试和说明而做的调整

- 从旧子模块复制 `jfk.wav`、`jfk.mp3` 到 `frontend/src-tauri/tests/fixtures/`；保留上游MIT许可证和原提交来源。两文件SHA256与清理前备份相同，WAV为16kHz单声道、11秒。
- `audio/import.rs` 的三个现有测试改用 `CARGO_MANIFEST_DIR` 定位样例；必需样例缺失会失败，消除了“目录不存在就跳过并通过”的行为。MP3 metadata仍允许引擎不提供帧数。
- fixtures局部 `.gitignore` 显式允许这个JFK WAV，防止仓库全局 `*.wav` 忽略规则使它无法进入提交；未放开用户录音。
- 移除 `frontend/package.json` 中指向不存在的 `electron/main.js` 的main字段，依赖与锁文件不变。
- 修正frontend README：不再声称当前Community已支持speaker diarization；本地/云摘要按所选provider说明。
- 更新CLAUDE.md，当前入口和历史恢复说明指向本记录。

## 3. 保留的活动兼容内容及后续退役条件

| 保留内容 | 原因 | 后续条件 |
|---|---|---|
| 原10个SQLx迁移，包括license/grace/speaker/summary backup | 存量库升级和checksum需要；本次逐文件比较HEAD未改变 | 永久保留已发布migration；新功能不依赖license状态 |
| 旧摘要JSON、IndexedDB与旧数据库导入/录音恢复 | 仍有调用方和历史数据读取价值 | 在P0旧数据fixture、实际迁移和恢复验收后才评估减少 |
| `whisper_engine.rs` 两个 `backend/whisper-server-package/models` 发现路径 | 发现的是用户外部模型目录，不是已删除的源码子模块；当前条件路径仍在执行 | 增加显式模型迁移后再移除，避免旧用户模型突然无法加载 |
| `api/api.rs` 注册的profile命令及旧HTTP helper | 仍在Tauri命令表里；仅“前端没搜到调用”不足以当作孤立文件删除 | P0做调用/外部IPC审计后退役并测试 |
| Sidebar的serverAddress/transcriptServerAddress和CSP旧端口 | serverAddress当前被多个组件作为初始化/加载门槛；直接删会阻断会议/设置加载 | P0换成native-ready状态，移除门槛后再收紧CSP和旧profile接口 |
| 当前录音、转写、内置LLM、FFmpeg、模型管理和安装脚本 | 属于受支持运行/构建路径 | 本次保留 |

因此本次完成的是**已确认归档路径清理**；并不声称所有带legacy或旧端口的活动代码已经退役。

## 4. 备份和恢复

清理前已打包所有待删除目录/文件，以及将修改的4个现有文件；先验证tar条目和hash，再开始删除。旧子模块当时无工作区改动。备份不含新方案文档。

- 本机备份目录：`/tmp/meetily-archive-cleanup-20261014-013313/`
- 备份文件：`before-cleanup.tar.gz`，9,889,117字节。
- SHA256：`8aa4dffaef083a4482ca92f14eaa4ca885fb252a15074b869f22cf7f40e0ae2b`。
- 备份清单：同目录 `manifest.json`；项目内机器清单另加删除路径和验证结果。
- 旧Whisper子模块commit：`d682e150908e10caa4c15883c633d7902d385237`。

`/tmp` 可能被系统清理，属于短期回滚材料；历史跟踪源码仍可从基线Git commit取回。需要长期备份时把tar和manifest一起复制到持久目录；不要把历史环境文件打包后上传公共仓库。

以下为**恢复示例，未执行**。只恢复指定归档目录，不覆盖当前整个工作树：

```bash
shasum -a 256 /tmp/meetily-archive-cleanup-20261014-013313/before-cleanup.tar.gz
tar -xzf /tmp/meetily-archive-cleanup-20261014-013313/before-cleanup.tar.gz \
  -C /Users/hlc/Code/meetily backend .gitmodules
```

恢复其他文件可从manifest选择准确条目；不要无选择地解压覆盖后续工作。恢复子模块需要父仓库原`.git/modules`仍存在；如果对象库已清理，用上述commit重新取得上游子模块。基于Git回退和基于tar恢复分别使用，不必混合执行。

## 5. 验证结果与限制

| 验证 | 结果 | 能证明什么 |
|---|---|---|
| `node --test .github/verify-portable-ggml.test.cjs` | **14/14通过** | 既有portable GGML配置检查逻辑保持正常，不代表已构建原生库 |
| `cargo metadata --offline --no-deps --format-version 1` | 通过；原有2条workspace警告仍存在 | Workspace成员/manifest解析正常；没有把删除目录加入Cargo入口 |
| 原10个迁移逐文件对比HEAD，再在SQLite内存库顺序执行 | **未改变，10/10执行成功**；foreign_key_check为空 | 迁移文本和基本schema有效；未代替实际存量库/SQLx运行迁移 |
| 删除路径/readback/活动module与include引用搜索 | 删除61路径，未发现活动module引用已移除Rust原型；音频测试旧引用已替换 | 源码结构范围检查，非完整编译证明 |
| 样例与tar逐字节比较、WAV头读取、忽略规则检查 | 通过 | 样例保存完整，WAV可读，进入新提交不被忽略 |
| Rust import.rs经rustfmt解析，JSON解析，`git diff --check` | 通过 | 修改的Rust语法/JSON结构/patch空白检查正常，不证明类型和运行行为 |
| `cargo test --offline -p meetily --lib test_extract_duration_from_metadata --no-default-features` | **阻断，未进入编译** | 缺 `ffmpeg-sidecar` Git依赖缓存；不能宣称音频测试通过 |
| 前端Bun tests/typecheck/build、完整Tauri打包、桌面录音运行 | 未执行 | 当前缺node_modules/Bun/sidecar，本次未安装或下载完整原生依赖 |

两条已有Cargo警告：子包 `[patch.crates-io]` 被workspace忽略、llama-helper的子包release profile被忽略。详细处理安排在[P0计划](superpowers/plans/2026-10-14-foundation.md)，本次没有直接移动配置而改变实际依赖。

提交清理前仍须在依赖齐备环境运行原生音频测试和当前前端/打包回归；源码/结构检查完成不等于运行验收。增强版的模型、文档renderer和HTTP/MCP能力均未实现，参见[总体设计](superpowers/specs/2026-10-14-internal-enhancement-design.md)。
