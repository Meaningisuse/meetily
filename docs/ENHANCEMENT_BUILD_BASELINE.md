# 内部增强版构建基线

验证日期：2026-10-14；起点 `4aeb587`，分支 `pro`。P0 → P3 → P4 → P1 → P2，内部自用，无收费授权改造。

## 本机与工具

| 项目 | 验证值 |
|---|---|
| 机器 | Apple M5，24 GiB RAM |
| 系统 | macOS 27.0.1，26A434 |
| 原生 SDK | Xcode 27.0，27A266a；完整 Xcode 必须可用 |
| Node | 24.14.0，`.nvmrc` 与 package engines |
| pnpm | 10.32.1，frontend/package.json 的 packageManager |
| Bun | 1.3.14，现有前端行为测试使用 |
| Rust/Cargo | 1.94.1，rust-toolchain.toml |
| CMake | 3.31.6 |

本次 pnpm frozen 安装成功。Bun/CMake 安装在 gitignored 的任务运行目录，不修改系统权限或全局版本；其他开发者须自行安装表中工具并加入 PATH。全局 xcode-select 当前指向 CommandLineTools，检查脚本在必要时只为当前进程选择 `/Applications/Xcode.app/Contents/Developer`。其他 Xcode 路径可显式设置 DEVELOPER_DIR。

```bash
pnpm -C frontend install --frozen-lockfile
scripts/check-enhancement.sh --frontend
scripts/check-enhancement.sh --native
scripts/check-enhancement.sh --all
scripts/check-enhancement.sh --bundle
```

`--all` 做 frozen install、Node 行为回归、Bun 测试、TypeScript、Next 静态构建、Cargo metadata、helper 准备及原生 lib tests。`--bundle` 先构建发布 helper，再由 Tauri build.rs 准备 FFmpeg 和打包本地 .app。该命令覆盖 createUpdaterArtifacts=false，避免要求生产更新签名私钥；不修改发行配置、不发布。首次原生构建需要联网下载锁定依赖及 FFmpeg；用户模型不嵌入包。任一步失败保留非零退出码，禁止把日志尾部命令的成功当作构建通过。

新增 enhancement-checks CI 在 macOS 上使用同一入口。现有五个发行 workflow 同步 Node/pnpm/Rust 版本。CI 配置已写入；本次没有运行 GitHub Actions，不能宣称远端构建已通过。

## 配置修整与问题证据

- 子包 `[patch.crates-io]` 原来被 Cargo 忽略。删除它们，保留锁文件实际使用的 registry cpal/esaxx；没有激活新 fork。
- 子包 release profile 原来被忽略。helper 的 codegen-units=1/opt-level=s 移到 workspace package override；未启用全局 LTO。
- llama.cpp 0.1.146 携带的 cpp-httplib 在 OpenSSL 4.0.3 下因 X509 返回 const 指针编译失败。helper 只从本地 GGUF/stdio 工作；通过 CMake 禁用未使用的可选 C++ HTTPS。Rust reqwest/provider TLS 保持可用。
- Xcode 27 对发布 proc-macro dylib 的符号剥离产生 `mis-aligned LINKEDIT string pool`，表现为找不到 phf_macros/zerofrom_derive。仅 build-override 设 strip=none 后发布编译成功；没有屏蔽编译或加载检查。
- 原设备测试调用完整检测链，恰好找到本机麦克风而得到 Wired，与 fallback 的 Unknown 断言冲突。改为直接验证名字 fallback，不依赖测试硬件。
- 原蓝牙超时用 mul_f32(2.0) 得到 159.999996ms。改为 saturating_mul(2)，已有 160ms 行为断言通过。

## 校验与结果

| 验证 | 结果 |
|---|---|
| frozen install | 成功，锁文件未变 |
| Bun frontend/tests | 45 pass，0 fail，158 assertions |
| Node portable GGML + runner | 17 pass，0 fail |
| TypeScript --noEmit | 通过 |
| Next 生产静态构建 | 通过，11 routes |
| Cargo metadata --locked | 两个 workspace 包正常，原 patch/profile 警告已消除 |
| Rust lib tests | 245 pass，0 fail，3 ignored |
| llama-helper debug/release + Metal | 编译成功 |
| 完整 Rust release | 编译成功 |
| Tauri .app 打包/启动 | 成功；原生窗口显示 Welcome to Meetily 与 Get Started，未进入录音或权限流程 |

忽略测试不计入通过数；尚未完成中文会议效果、120分钟录音、Windows 发行验收。

锁文件 SHA256：

- Cargo.lock：`decd8ac805b3c3ad512da887e29c6dfe5db7dda858cb961eda1833181d5a23fe`
- frontend/pnpm-lock.yaml：`84b4c57725d224ef1c687de593e23015bef76b367b82eb6689b173a07a7b94d1`

已有十个 SQLx migration 原文未修改；升级前后须核对原文件 diff/checksum。新增迁移不能插入已发布编号之前。

## CPU / Metal 实际推理

新增 `frontend/src-tauri/examples/whisper_smoke.rs` 使用仓库已有 MIT 许可 JFK WAV；失败、不含预期 speech、缺模型均非0退出。两次运行同一带 Metal 的二进制，分别显式设置 use_gpu=false/true；这是运行路径验收，不代表另一个 CPU-only 安装包。

使用 [whisper.cpp 官方模型说明](https://github.com/ggml-org/whisper.cpp/blob/master/models/README.md) 指向的 `ggerganov/whisper.cpp` tiny.en 模型。模型 SHA1 与官方列表一致：`c78c86eb1a8faa21b369bcd33207cc90d64ae9df`。本地下载不纳入 Git。

```bash
cargo run --locked -p meetily --example whisper_smoke -- /path/to/ggml-tiny.en.bin cpu
cargo run --locked -p meetily --example whisper_smoke -- /path/to/ggml-tiny.en.bin metal
```

11,000ms 样例：CPU 742ms、Metal 3,363ms（均含模型初始化），文本相同；输出两个原始模型时间段 0–8,000ms 与 8,000–11,000ms。小样例不用于比较长期性能，也不用于中文说话人准确率结论。完整会议性能由后续 P4 验收报告独立记录。
