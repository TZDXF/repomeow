-- App version: 0.1.13
-- Status: in development

-- AI 用量日志增加「缓存命中 tokens」列:OpenAI 兼容接口的
-- prompt_tokens_details.cached_tokens(ACP 为 cachedReadTokens),
-- 是输入 tokens 的子集,单独记录用于观察提示词缓存命中率
ALTER TABLE ai_usage_log ADD COLUMN cached_tokens INTEGER;
