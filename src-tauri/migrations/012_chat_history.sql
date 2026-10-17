-- App version: 0.2.11
-- Status: in development

-- 展示时间线与完整 Agent 上下文一并保存,按归一化项目路径隔离。
CREATE TABLE IF NOT EXISTS chat_history (
    id TEXT NOT NULL,
    project_path TEXT NOT NULL,
    title TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    state TEXT NOT NULL,
    context TEXT NOT NULL,
    PRIMARY KEY (project_path, id)
);
CREATE INDEX IF NOT EXISTS idx_chat_history_project_updated
    ON chat_history(project_path, updated_at DESC);

-- NULL 表示用户已新建空会话,重启后不自动打开上一段历史。
CREATE TABLE IF NOT EXISTS chat_active_sessions (
    project_path TEXT PRIMARY KEY,
    session_id TEXT
);
