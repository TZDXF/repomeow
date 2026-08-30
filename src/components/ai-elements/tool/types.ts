// 本地类型定义:替代 AI SDK `ai` 包的 ToolUIPart / DynamicToolUIPart。
// RepoMeow 的工具调用由 Rust 后端 chat_send 事件流推送,生成组件保持纯展示。
export type ToolState = "input-streaming" | "input-available" | "output-available" | "output-error";
