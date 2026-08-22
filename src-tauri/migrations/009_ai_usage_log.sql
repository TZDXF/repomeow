-- App version: 0.1.13
-- Status: in development

-- AI 模型用量日志:每条记录一次 LLM 调用(任务类型 + token 消耗)。
-- 覆盖三条链路:前端内置 API(ai.ts)、Rust 定时报告(scheduler.rs)、
-- 本地 coding agent 后端(ACP,记录每次 PromptResponse.usage)。
-- token 列可空:provider 未返回 usage 时行仍在(调用次数可统计),但不计入汇总求和
CREATE TABLE ai_usage_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    task_type TEXT NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER,
    output_tokens INTEGER,
    total_tokens INTEGER,
    duration_ms INTEGER
);

CREATE INDEX idx_ai_usage_log_created_at ON ai_usage_log (created_at);
