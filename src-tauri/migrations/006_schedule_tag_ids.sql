-- App version: 0.1.13
-- Status: in development

-- 定时任务按标签动态包含项目:tag_ids 为 JSON 数组,与 project_ids 并存取并集;
-- 执行时反查带有任一选中标签的未归档项目,新项目打上标签即自动纳入
ALTER TABLE report_schedules ADD COLUMN tag_ids TEXT NOT NULL DEFAULT '[]';
