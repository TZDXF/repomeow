import { createRenderer, defineComponent, h, type Component } from "vue";
import { describe, expect, it, vi } from "vitest";
import UpdateNotes from "./UpdateNotes";

const { loaderState } = vi.hoisted(() => ({
  loaderState: {
    load: (): Promise<Component> => Promise.reject(new Error("loader 未初始化")),
  },
}));

const MarkdownStub = defineComponent({
  props: {
    content: { type: String, default: "" },
  },
  setup: (props) => () => h("div", { "data-safe-markdown": "" }, props.content),
});

vi.mock("vue-i18n", () => ({
  useI18n: () => ({
    t: (key: string) => (key === "update.notesLoading" ? "正在加载更新说明..." : key),
  }),
}));
vi.mock("./update-markdown-loader", () => ({
  loadUpdateMarkdown: () => loaderState.load(),
}));

interface TestNode {
  type: "element" | "text" | "comment";
  tag: string;
  text: string;
  props: Record<string, unknown>;
  children: TestNode[];
  parent: TestNode | null;
}

function createNode(type: TestNode["type"], options: Partial<TestNode> = {}): TestNode {
  return {
    type,
    tag: "",
    text: "",
    props: {},
    children: [],
    parent: null,
    ...options,
  };
}

function insertNode(child: TestNode, parent: TestNode, anchor?: TestNode | null) {
  if (child.parent) {
    const siblings = child.parent.children;
    siblings.splice(siblings.indexOf(child), 1);
  }
  child.parent = parent;
  const anchorIndex = anchor ? parent.children.indexOf(anchor) : -1;
  if (anchorIndex < 0) {
    parent.children.push(child);
  } else {
    parent.children.splice(anchorIndex, 0, child);
  }
}

const renderer = createRenderer<TestNode, TestNode>({
  patchProp: (element, key, _previous, next) => {
    element.props[key] = next;
  },
  insert: insertNode,
  remove: (child) => {
    if (!child.parent) {
      return;
    }
    const siblings = child.parent.children;
    siblings.splice(siblings.indexOf(child), 1);
    child.parent = null;
  },
  createElement: (tag) => createNode("element", { tag }),
  createText: (text) => createNode("text", { text }),
  createComment: (text) => createNode("comment", { text }),
  setText: (node, text) => {
    node.text = text;
  },
  setElementText: (element, text) => {
    const child = createNode("text", { text, parent: element });
    element.children = text ? [child] : [];
  },
  parentNode: (node) => node.parent,
  nextSibling: (node) => {
    const siblings = node.parent?.children;
    if (!siblings) {
      return null;
    }
    return siblings[siblings.indexOf(node) + 1] ?? null;
  },
  insertStaticContent: (content, parent, anchor) => {
    const node = createNode("text", { text: content });
    insertNode(node, parent, anchor);
    return [node, node];
  },
});

function textContent(node: TestNode): string {
  if (node.type !== "element") {
    return node.type === "comment" ? "" : node.text;
  }
  return node.children.map(textContent).join("");
}

function renderNotes(content: string) {
  const root = createNode("element", { tag: "root" }),
    app = renderer.createApp(h(UpdateNotes, { content }));
  app.mount(root);
  return { root, unmount: () => app.unmount() };
}

describe("UpdateNotes 异步渲染", () => {
  it("loader 延迟时显示明确提示，完成后恢复 Markdown 渲染", async () => {
    let resolveLoader!: (component: Component) => void;
    loaderState.load = () =>
      new Promise((resolve) => {
        resolveLoader = resolve;
      });
    const { root, unmount } = renderNotes("**延迟加载的更新说明**");

    try {
      await vi.waitFor(() => {
        expect(textContent(root)).toContain("正在加载更新说明...");
      });
      expect(textContent(root)).not.toContain("延迟加载的更新说明");

      resolveLoader(MarkdownStub);
      await vi.waitFor(() => {
        expect(textContent(root)).toContain("**延迟加载的更新说明**");
      });
      expect(textContent(root)).not.toContain("正在加载更新说明...");
    } finally {
      unmount();
    }
  });

  it("loader reject 时以可读的纯文本完整展示原始更新说明", async () => {
    const rawNotes = "# 降级标题\n\n**加粗原文** 与 [链接](https://example.com)";
    loaderState.load = () => Promise.reject(new Error("模拟 Markdown chunk 加载失败"));
    const { root, unmount } = renderNotes(rawNotes);

    try {
      await vi.waitFor(() => {
        expect(textContent(root)).toContain(rawNotes);
      });
      expect(textContent(root)).not.toContain("正在加载更新说明...");
      expect(textContent(root)).toContain("**加粗原文**");
      expect(textContent(root)).toContain("[链接](https://example.com)");
    } finally {
      unmount();
    }
  });
});
