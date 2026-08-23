-- App version: 0.1.13
-- Status: in development

-- 应用内置定时任务配置。内置任务只允许启停和调整间隔，不允许删除。
CREATE TABLE IF NOT EXISTS system_schedules (
    id               TEXT PRIMARY KEY,
    enabled          INTEGER NOT NULL DEFAULT 1,
    interval_minutes INTEGER NOT NULL,
    last_run_at      INTEGER
);

INSERT OR IGNORE INTO system_schedules (id, enabled, interval_minutes, last_run_at)
VALUES ('git_update', 1, 10, NULL);
