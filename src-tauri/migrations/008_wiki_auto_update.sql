-- App version: 0.1.13
-- Status: in development

-- 项目级「Wiki 自动增量更新」开关:全局开关关闭时,仅勾选了此开关的项目参与
-- wiki 自动更新全局开关打开时所有项目都参与,勾选被忽略。
-- 与 auto_pull 相互独立;本地 HEAD 更新且未同步进 wiki 的提交数达到全局阈值
-- (stores/settings.ts 的 wikiAutoUpdateThreshold,默认 10)时自动执行增量更新
ALTER TABLE projects ADD COLUMN wiki_auto_update INTEGER NOT NULL DEFAULT 0;
