import { createRenderer, h, nextTick, type Component, type Ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
// 测试当前安装的 vue-stream-markdown 1.1.0;升级时同步内部入口并复测节点身份。
// @ts-expect-error 上游内部组件没有声明文件
import Code from "../../node_modules/vue-stream-markdown/dist/code-TE80M-uF.js";

const { tokenize, runtime } = vi.hoisted(() => ({
  tokenize: vi.fn(),
  runtime: { installed: undefined as Ref<boolean> | undefined },
}));
vi.mock("../../node_modules/vue-stream-markdown/dist/composables-8Bz3Jjzb.js", async () => {
  const { ref } = await import("vue");
  runtime.installed = ref(true);
  return {
    C: () => ({
      cdnOptions: ref<undefined>(),
      codeOptions: ref<undefined>(),
      isDark: ref(false),
      shikiOptions: ref<undefined>(),
    }),
    T: () => ({ showLineNumbers: ref(true) }),
    a: () => ({ codeToTokens: tokenize, installed: runtime.installed, getShiki: vi.fn() }),
  };
});

interface HostNode {
  tag: string;
  text: string;
  children: HostNode[];
  parent: HostNode | null;
  props: Record<string, unknown>;
}
const node = (tag: string, text = ""): HostNode => ({
  tag,
  text,
  children: [],
  parent: null,
  props: {},
});
function harness() {
  const removed: HostNode[] = [];
  const renderer = createRenderer<HostNode, HostNode>({
    createElement: (tag) => node(tag),
    createText: (text) => node("text", text),
    createComment: (text) => node("comment", text),
    setText: (el, text) => {
      el.text = text;
    },
    setElementText: (el, text) => {
      el.text = text;
      el.children = [];
    },
    parentNode: (el) => el.parent,
    nextSibling: (el) => el.parent?.children[el.parent.children.indexOf(el) + 1] ?? null,
    // Vue RendererOptions 的固定宿主接口签名。
    // oxlint-disable-next-line max-params
    patchProp: (el, key, _old, value) => {
      el.props[key] = value;
    },
    insert(el, parent, anchor) {
      el.parent = parent;
      const index = anchor ? parent.children.indexOf(anchor) : -1;
      if (index < 0) {
        parent.children.push(el);
      } else {
        parent.children.splice(index, 0, el);
      }
    },
    remove(el) {
      removed.push(el);
      el.parent?.children.splice(el.parent.children.indexOf(el), 1);
      el.parent = null;
    },
  });
  const root = node("root");
  return {
    root,
    removed,
    render(value: string, lang = "js") {
      renderer.render(
        h(Code as Component, {
          node: { type: "code", value, lang },
          showHeader: false,
          markdownParser: {},
          nodeRenderers: {},
          nodeKey: "code-0",
          deep: 0,
        }),
        root,
      );
    },
    unmount: () => renderer.render(null, root),
  };
}
function deferred() {
  let resolve!: (value: unknown) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const tokens = (content: string) => ({
  tokens: [[{ content, htmlStyle: { color: "#ff0000" } }]],
  fg: "#222",
});
const text = (el: HostNode): string => el.text + el.children.map(text).join("");
async function flush() {
  await Promise.resolve();
  await nextTick();
}
beforeEach(() => {
  tokenize.mockReset();
  runtime.installed!.value = true;
});

describe("Markdown 代码块稳定渲染", () => {
  it("冷启动立即显示代码,高亮前后复用 pre/code/行节点,不卸载内容", async () => {
    const pending = deferred();
    tokenize.mockReturnValue(pending.promise);
    const view = harness();
    view.render("const a = 1");
    const pre = view.root.children[0]!.children[0]!;
    const code = pre.children[0]!;
    const line = code.children[0]!;
    expect(pre.tag).toBe("pre");
    expect(text(pre)).toBe("const a = 1");
    pending.resolve(tokens("const a = 1"));
    await flush();
    expect(view.root.children[0]!.children[0]).toBe(pre);
    expect(pre.children[0]).toBe(code);
    expect(code.children[0]).toBe(line);
    expect(line.children[0]?.props.style).toEqual({ color: "#ff0000" });
    expect(view.removed).toEqual([]);
    view.unmount();
  });

  it("流式更新立即显示新代码,忽略乱序返回的旧高亮", async () => {
    const first = deferred();
    const second = deferred();
    tokenize.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const view = harness();
    view.render("old");
    const pre = view.root.children[0]!.children[0]!;
    view.render("new");
    await nextTick();
    expect(text(pre)).toBe("new");
    second.resolve(tokens("new"));
    await flush();
    first.resolve(tokens("old"));
    await flush();
    expect(text(pre)).toBe("new");
    expect(view.root.children[0]!.children[0]).toBe(pre);
    expect(view.removed).toEqual([]);
    view.unmount();
  });

  it("高亮运行时从未就绪变为就绪时不替换代码 DOM", async () => {
    runtime.installed!.value = false;
    tokenize.mockResolvedValueOnce(tokens("hello"));
    const view = harness();
    view.render("hello");
    const wrapper = view.root.children[0]!;
    const pre = wrapper.children[0]!;
    expect(text(pre)).toBe("hello");
    expect(tokenize).not.toHaveBeenCalled();
    runtime.installed!.value = true;
    await nextTick();
    await flush();
    expect(tokenize).toHaveBeenCalledTimes(1);
    expect(view.root.children[0]).toBe(wrapper);
    expect(wrapper.children[0]).toBe(pre);
    expect(text(pre)).toBe("hello");
    expect(view.removed).toEqual([]);
    view.unmount();
  });

  it("卸载后完成高亮不会重新插入代码 DOM", async () => {
    const pending = deferred();
    tokenize.mockReturnValue(pending.promise);
    const view = harness();
    view.render("hello");
    view.unmount();
    pending.resolve(tokens("hello"));
    await flush();
    expect(view.root.children).toEqual([]);
  });
});
