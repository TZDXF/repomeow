-- App version: 0.1.12
-- Status: in development

-- 项目「跟踪更新」开关:开启后后台循环定时 fetch,远端有更新(落后 upstream)时
-- 尝试 `git merge --ff-only @{u}` 快进;分叉/本地改动会被覆盖等无法快进的情形
-- git 直接拒绝且不留合并状态,即「有冲突则取消」,全程静默不提醒
ALTER TABLE projects ADD COLUMN auto_pull INTEGER NOT NULL DEFAULT 0;
