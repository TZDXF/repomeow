import { createSSRApp, h } from "vue";
import { renderToString } from "vue/server-renderer";
import { describe, expect, it, vi } from "vitest";
import SafeMarkdown from "./SafeMarkdown.vue";

// 保留真实 SFC 的 prop 转换与配置合并,只替换底层渲染器以检查实际收到的配置。
vi.mock("vue-stream-markdown", async () => {
  const { defineComponent, h } = await import("vue");
  return {
    Markdown: defineComponent({
      props: ["controls", "previewers", "extensions"],
      setup: (props) => () => h("pre", JSON.stringify(props)),
    }),
  };
});

async function renderConfig(props: Record<string, unknown> = {}) {
  const html = await renderToString(createSSRApp(() => h(SafeMarkdown, props)));
  const json = html.match(/<pre[^>]*>(.*?)<\/pre>/s)?.[1];
  if (!json) throw new Error("未收到 Markdown 渲染配置");
  return JSON.parse(json.replace(/&quot;/g, '"').replace(/&amp;/g, "&"));
}

describe("SafeMarkdown 默认预览", () => {
  it("未传可选布尔参数时保留预览与工具栏,并为 Mermaid 注册渲染扩展", async () => {
    const config = await renderConfig();
    expect(config.previewers).toEqual({ html: { sandbox: "" } });
    expect(config.controls).toBe(true);
    expect(config.extensions).toHaveProperty("mermaid");
  });

  it("保留调用方显式关闭预览与工具栏的配置", async () => {
    const config = await renderConfig({ previewers: false, controls: false });
    expect(config.previewers).toBe(false);
    expect(config.controls).toBe(false);
  });

  it("自定义预览选项仍保留 HTML 空沙箱限制", async () => {
    const config = await renderConfig({
      previewers: { placement: "center", html: { sandbox: "allow-scripts" } },
      controls: { code: { copy: true } },
    });
    expect(config.previewers).toEqual({ placement: "center", html: { sandbox: "" } });
    expect(config.controls).toEqual({ code: { copy: true } });
  });
});
