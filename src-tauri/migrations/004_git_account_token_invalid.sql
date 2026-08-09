-- App version: 0.1.9
-- Status: in development

-- 账号 Token 失效标记:拉取仓库遇到 401 时置 1,设置页账号列表据此显示「Token 已失效」;
-- 重新验证通过(更新 Token/实例地址)时清 0
ALTER TABLE git_accounts ADD COLUMN token_invalid INTEGER NOT NULL DEFAULT 0;
