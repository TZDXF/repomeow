import { createSSRApp, defineComponent, h, type Component } from "vue";
import { renderToString } from "vue/server-renderer";
import { beforeEach, describe, expect, it, vi } from "vitest";
import UpdateDialog from "./UpdateDialog.vue";

const { store, loaderState } = vi.hoisted(() => ({
  store: {
    update: null as null | { body?: string; version: string },
    currentVersion: "1.0.0",
    dialogOpen: true,
    status: "available",
    progress: 0,
    error: "",
    downloadAndInstall: vi.fn(),
    relaunchApp: vi.fn(),
  },
  loaderState: {
    load: (): Promise<Component> => Promise.reject(new Error("loader 未初始化")),
  },
}));

const MarkdownStub = defineComponent({
  props: {
    content: { type: String, default: "" },
    mode: { type: String, default: "static" },
  },
  setup: (props) => () =>
    h("div", { "data-safe-markdown": "", "data-mode": props.mode }, props.content),
});

vi.mock("@/stores/update", () => ({ useUpdateStore: () => store }));
vi.mock("vue-i18n", () => ({
  useI18n: () => ({
    t: (key: string) => (key === "update.noNotes" ? "暂无更新说明" : key),
  }),
}));
vi.mock("./update-markdown-loader", () => ({
  loadUpdateMarkdown: () => loaderState.load(),
}));
vi.mock("@/components/common/ScrollArea.vue", async () => {
  const { defineComponent, h } = await import("vue");
  return {
    default: defineComponent({
      setup:
        (_, { slots }) =>
        () =>
          h("div", slots.default?.()),
    }),
  };
});
vi.mock("@/components/ui/button", async () => {
  const { defineComponent, h } = await import("vue");
  return {
    Button: defineComponent({
      setup:
        (_, { slots }) =>
        () =>
          h("button", slots.default?.()),
    }),
  };
});
vi.mock("@/components/ui/dialog", async () => {
  const { defineComponent, h } = await import("vue"),
    passthrough = defineComponent({
      setup:
        (_, { slots }) =>
        () =>
          h("div", slots.default?.()),
    });
  return {
    Dialog: passthrough,
    DialogContent: passthrough,
    DialogDescription: passthrough,
    DialogFooter: passthrough,
    DialogHeader: passthrough,
    DialogTitle: passthrough,
  };
});

async function renderDialog() {
  return renderToString(createSSRApp(() => h(UpdateDialog)));
}

describe("UpdateDialog 更新说明", () => {
  beforeEach(() => {
    store.update = null;
    loaderState.load = () => Promise.resolve(MarkdownStub);
  });

  it("将常见 Markdown 格式交给现有安全组件静态渲染", async () => {
    store.update = {
      version: "1.1.0",
      body: [
        "# 新功能",
        "",
        "**加粗**、*斜体* 与 `内联代码`",
        "",
        "- 第一项",
        "- 第二项",
        "",
        "[发布页面](https://example.com/release)",
      ].join("\n"),
    };

    const html = await renderDialog();
    expect(html).toContain("data-safe-markdown");
    expect(html).toContain('data-mode="static"');
    expect(html).toContain("# 新功能");
    expect(html).toContain("**加粗**、*斜体* 与 `内联代码`");
    expect(html).toContain("- 第一项");
    expect(html).toContain("[发布页面](https://example.com/release)");
  });

  it.each(["", "  \n\t  "])("空内容保留暂无说明提示: %j", async (body) => {
    store.update = { version: "1.1.0", body };

    const html = await renderDialog();
    expect(html).toContain("暂无更新说明");
  });

  it("长内容完整保留在限高滚动区域内", async () => {
    const lastItem = "最后一条更新说明";
    store.update = {
      version: "1.1.0",
      body: Array.from({ length: 80 }, (_, index) => `- 第 ${index + 1} 条更新说明`)
        .concat(`- ${lastItem}`)
        .join("\n"),
    };

    const html = await renderDialog();
    expect(html).toContain("max-h-64");
    expect(html).toContain("第 1 条更新说明");
    expect(html).toContain(lastItem);
  });
});
