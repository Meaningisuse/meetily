# P1 模板管理 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. 本次只交付计划；实施时遵循用户已授权的执行范围。

**Goal:** 在六个现有模板和JSON schema上实现可持久化模板管理，不新增付费门槛。

**Architecture:** 保留loader读取兼容，增加有版本的模板服务和章节编辑器；选中模板在会议数据库引用，生成冻结内容。

**Tech Stack:** Rust文件IO、serde、现有Template、React设置界面、Bun tests。

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

- 内置模板与用户同ID文件：不可覆盖或静默丢失用户提示词。
- 路径穿越、符号链接与并发编辑：限定目录、expected_revision冲突。
- 删除正被会议选择的模板：指定替代模板，历史摘要不变化。
- 编辑模板时摘要正在生成：任务使用冻结版本，结果显示来源。
- 写到一半/磁盘满：保留旧文件，不出现半个JSON。

## Task T1：模板服务与内容版本

**Files:** 新增 `summary/templates/{service,storage}.rs`；修改 `summary/templates/{types,loader,mod}.rs`、`summary/template_commands.rs`；新增 `20261014000600_add_template_preferences.sql`、`database/repositories/template_preference.rs`。

**Interfaces:** `TemplateRecord` 按设计；`TemplateService::{list,get,create,update,duplicate,delete,import_json,export_json}`。`update(id: &str, content: Template, expected_revision: &str) -> Result<TemplateRecord, EnhancementError>`；`delete(id, expected_revision, replacement_id)`；`import_json(raw: &[u8])` 总是产生新custom UUID。

- [ ] 写 `summary::templates::tests::crud_preserves_builtin_and_detects_conflict`：6内置均可读且不可写；复制/编辑/删除custom成功；过期hash返回CONFLICT。
- [ ] 写 `summary::templates::tests::rejects_path_escape_and_partial_write`：`../`、绝对路径、符号链接越界、129KiB、65sections、非法format拒绝；rename前注入失败仍可读旧版本。
- [ ] 先运行 `cargo test -p meetily --lib crud_preserves_builtin_and_detects_conflict` 和第二个test，再实现限制、SHA256、temp+fsync+rename；测试目录注入避免修改真实用户模板。
- [ ] 实现七个commands并登记lib.rs；保留api_list_templates/api_get_template_details作为兼容wrapper；JSON重导入可读，revision不作为文件名。
- [ ] 跑模板全套 `cargo test -p meetily --lib summary::templates`，审阅提交 `feat: add versioned custom template service`。

## Task T2：模板编辑器与会议选择

**Files:** 新增 `frontend/src/components/Templates/{TemplateManager,TemplateEditor}.tsx`、`frontend/src/hooks/useTemplateManager.ts`、`frontend/tests/hooks/template-manager.test.tsx`；修改 `ModelSettingsModal.tsx`、`hooks/meeting-details/useTemplates.ts`、类型和摘要选择入口。

**Interfaces:** hook提供 `{records,selectedId,loading,error,create,update,duplicate,remove,importJson,exportJson}`，后端保存成功再更新state；会议选择 `api_set_meeting_template(meeting_id,template_id,expected_revision)` 返回SourceVersions。

- [ ] 写 `template_manager_saves_full_instructions_and_reloads`：编辑instruction/item_format，重新mount返回同内容；取消无写入；失败保持dirty输入和错误。
- [ ] 写 `deleting_selected_template_requires_replacement`：未选替代不可确认，成功切standard_meeting，已生成摘要原文不变。
- [ ] `bun test frontend/tests/hooks/template-manager.test.tsx` 先失败，再实现列表、复制、JSON交换和章节编辑器；缺少模板显示可恢复错误。
- [ ] 生成时把TemplateRecord完整内容/hash交给已有fingerprint，写 `summary::tests::template_edit_during_generation_uses_snapshot` 验证原任务不会中途换prompt。
- [ ] 运行Bun+Rust对应tests、typecheck；手工重启app验证选择和模板；提交 `feat: add template manager and persistent selection`。

## Task T3：旧自定义模板导入与回归

**Files:** 新增 `summary/templates/migration.rs`、`frontend/tests/hooks/template-migration.test.tsx`；修改TemplateManager和模板服务；更新使用文档。

**Interfaces:** `preview_legacy_templates() -> Vec<LegacyTemplateCandidate>`，`migrate_legacy_templates(selected_ids: Vec<String>) -> TemplateMigrationReport`。报告包括原ID、新ID、备份路径、失败原因；默认只预览。

- [ ] 写 `summary::templates::tests::legacy_collision_preserves_file_and_remaps_selection`：旧standard_meeting自定义覆盖→新custom ID；原文件备份完整，所选meeting引用迁移，原覆盖文件移入backup后内置仍原内容；未迁移前明确显示legacy override并保持旧选择语义。
- [ ] 写 `legacy_migration_requires_user_action`：页面展示预览，未点击迁移无文件写入。
- [ ] 分别运行Rust filter/Bun test先失败，再实现逐文件带journal的可恢复迁移；重复迁移基于原hash不产生多份副本；中途失败恢复引用/原文件。
- [ ] 验证六模板、名称中文、导入非法JSON、删除引用、模板空目录/只读目录，记录内部演示；提交 `feat: migrate legacy template overrides safely`。

**P1完成条件：** 六内置、完整CRUD/JSON交换、选择重启持久化、生成来源冻结、旧文件保护。Markdown完整交换不在此包承诺。
