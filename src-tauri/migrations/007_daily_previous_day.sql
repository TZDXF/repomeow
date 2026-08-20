-- App version: 0.1.13
-- Status: in development

-- 日报生成范围可选:1 = 前一天(次日生成,默认),0 = 当天;星期过滤按报告日判定
ALTER TABLE report_schedules ADD COLUMN previous_day INTEGER NOT NULL DEFAULT 1;
