import path from "node:path";
import { defineConfig } from "vitest/config";

// 独立的 vitest 配置:复用 vite 的 @ alias,避免在生产构建中加载测试相关配置
// https://vitest.dev/config/
export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "node",
    // 渲染回归测试需要 mock Markdown 库内部的异步高亮接口。
    server: { deps: { inline: ["vue-stream-markdown"] } },
    include: ["src/**/*.test.ts"],
    // 最小 mock 即可:仅覆盖测试需要的模块,不引入 happy-dom 等浏览器环境
    setupFiles: [],
  },
});
