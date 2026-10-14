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

F2验证：Rust253 pass / 0 fail / 3 ignored；前端50 pass / 0 fail，Node17 pass；类型检查、Next生产构建通过。Rust/TypeScript读取同一个公共JSON fixture。迁移回归先在原十个迁移后填旧数据，再升级，检查文本、时间、笔记JSON与外键删除行为。没有对用户历史会议运行删除或效果验收。

## 接下来实施

- P0/F3：持久、有界后台任务，取消、重启中断及摘要来源冲突。
- P0/F4：数据库恢复材料保护、有界实时积压与补处理、全provider中文token预算。
- P3/A1–A5：loopback授权gateway、只读与后续写入/录音、CLI/MCP、事务事件Webhook。
- P4/D1–D5：本机模型/时间探针，真实时间映射、版本化分离结果和人工纠正。用户确认使用本机；带说话人标注的中文会议音频仍缺，不能据公共样例宣称DER或中文业务效果通过。

P0整包、P3、P4目前尚未完成。P1模板管理与P2导出均未开始。
