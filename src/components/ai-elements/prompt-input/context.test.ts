import { describe, expect, it, vi } from "vitest";
import { createSSRApp } from "vue";
import { renderToString } from "vue/server-renderer";
import { usePromptInputProvider } from "./context";
import type { PromptInputContext, PromptInputMessage } from "./types";

async function harness(
  onSubmit: (message: PromptInputMessage) => void | boolean | Promise<void | boolean>,
) {
  let context!: PromptInputContext;
  await renderToString(
    createSSRApp({
      setup() {
        context = usePromptInputProvider({ initialInput: "待发送的问题", onSubmit });
        return () => null;
      },
    }),
  );
  return context;
}

describe("prompt input 提交结果", () => {
  it("拒绝发送时恢复输入并保留附件", async () => {
    const submit = vi.fn(() => false);
    const context = await harness(submit);
    context.files.value = [{ id: "f1", type: "file", url: "data:text/plain,a", filename: "a.txt" }];
    await context.submitForm();
    expect(submit).toHaveBeenCalledWith({ text: "待发送的问题", files: context.files.value });
    expect(context.textInput.value).toBe("待发送的问题");
    expect(context.files.value).toHaveLength(1);
    expect(context.isLoading.value).toBe(false);
  });

  it("异步拒绝不覆盖等待期间的新输入", async () => {
    let resolve!: (value: boolean) => void;
    const result = new Promise<boolean>((done) => {
      resolve = done;
    });
    const context = await harness(() => result);
    const pending = context.submitForm();
    expect(context.textInput.value).toBe("");
    context.setTextInput("新草稿");
    resolve(false);
    await pending;
    expect(context.textInput.value).toBe("新草稿");
  });

  it("异步拒绝且未输入新草稿时恢复原问题", async () => {
    const context = await harness(async () => false);
    await context.submitForm();
    expect(context.textInput.value).toBe("待发送的问题");
  });

  it.each([true, undefined])("兼容成功/无返回值的提交回调(%s),清空已提交内容", async (result) => {
    const context = await harness(() => result);
    context.files.value = [{ id: "f1", type: "file", url: "data:text/plain,a" }];
    await context.submitForm();
    expect(context.textInput.value).toBe("");
    expect(context.files.value).toEqual([]);
  });
});
