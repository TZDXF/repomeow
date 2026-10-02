import { beforeEach, describe, expect, it, vi } from "vitest";
import { ref } from "vue";
import { toast } from "vue-sonner";
import { cmd } from "@/lib/tauri";
import { getCachedTranslation, putCachedTranslation } from "@/lib/translation-cache";
import { useMarkdownTranslation } from "./useMarkdownTranslation";

vi.mock("vue-sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));
vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));
vi.mock("@/lib/translation-cache", () => ({
  getCachedTranslation: vi.fn(),
  putCachedTranslation: vi.fn(() => Promise.resolve()),
}));

const cmdMock = vi.mocked(cmd);
const getCachedMock = vi.mocked(getCachedTranslation);
const toastError = vi.mocked(toast.error);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** 可切换身份/正文的翻译实例(模拟文件切换) */
function setup(identity = "a.md", text = "hello") {
  const currentIdentity = ref<string | null>(identity);
  const currentText = ref<string | null>(text);
  const translation = useMarkdownTranslation({
    getIdentity: () => currentIdentity.value,
    getText: () => currentText.value,
    getLanguage: () => "zh-CN",
  });
  return { translation, currentIdentity, currentText };
}

/** 每次 ai_translate_markdown 调用登记一个 deferred;ai_cancel_run 立即成功 */
function mockAiCalls() {
  const calls: ReturnType<typeof deferred<string | null>>[] = [];
  cmdMock.mockImplementation((name: string) => {
    if (name === "ai_translate_markdown") {
      const d = deferred<string | null>();
      calls.push(d);
      return d.promise as Promise<never>;
    }
    return Promise.resolve(undefined as never);
  });
  return calls;
}

/** 等待第 n 个 AI 请求真正发出(过了缓存查询阶段) */
async function waitAiCall(calls: unknown[], n: number) {
  await vi.waitFor(() => expect(calls.length).toBe(n));
}

beforeEach(() => {
  vi.clearAllMocks();
  getCachedMock.mockResolvedValue(null);
});

describe("useMarkdownTranslation", () => {
  it("缓存 await 期间置忙防重复触发;再点取消后晚到的缓存命中作废", async () => {
    const cacheQuery = deferred<string | null>();
    getCachedMock.mockReturnValue(cacheQuery.promise);
    const { translation } = setup();

    const pending = translation.toggle();
    // 缓存查询期间已是 busy:再点 = 取消本轮
    expect(translation.translating.value).toBe(true);
    await translation.toggle();
    expect(translation.translating.value).toBe(false);

    // 晚到的缓存命中作废:不展示、不置忙、不再调 AI
    cacheQuery.resolve("cached-text");
    await pending;
    expect(translation.translatedText.value).toBeNull();
    expect(translation.showTranslated.value).toBe(false);
    expect(cmdMock).not.toHaveBeenCalledWith("ai_translate_markdown", expect.anything());
  });

  it("缓存命中(未取消)时展示译文且不发起 AI 请求", async () => {
    getCachedMock.mockResolvedValue("cached-text");
    const { translation } = setup();
    await translation.toggle();
    expect(translation.translatedText.value).toBe("cached-text");
    expect(translation.translatedFor.value).toBe("a.md");
    expect(translation.showTranslated.value).toBe(true);
    expect(translation.translating.value).toBe(false);
    expect(cmdMock).not.toHaveBeenCalledWith("ai_translate_markdown", expect.anything());
  });

  it("AI 翻译成功:展示译文并回填缓存", async () => {
    const calls = mockAiCalls();
    const { translation } = setup();
    const pending = translation.toggle();
    await waitAiCall(calls, 1);
    calls[0].resolve("译文");
    await pending;
    expect(translation.translatedText.value).toBe("译文");
    expect(translation.showTranslated.value).toBe(true);
    expect(translation.translating.value).toBe(false);
    expect(putCachedTranslation).toHaveBeenCalledWith("hello", "zh-CN", "译文");
  });

  it("切换文件(reset)后:发 ai_cancel_run,旧请求成功返回被丢弃", async () => {
    const calls = mockAiCalls();
    const { translation, currentIdentity } = setup();

    const pending = translation.toggle();
    await waitAiCall(calls, 1);
    expect(translation.translating.value).toBe(true);

    // 换文件:取消在途请求并清空状态
    translation.reset();
    currentIdentity.value = "b.md";
    expect(translation.translating.value).toBe(false);
    expect(cmdMock).toHaveBeenCalledWith("ai_cancel_run", {
      runId: expect.stringContaining("translate-"),
    });

    calls[0].resolve("迟到的译文");
    await pending;
    expect(translation.translatedText.value).toBeNull();
    expect(translation.showTranslated.value).toBe(false);
    expect(translation.hasTranslation.value).toBe(false);
    expect(toastError).not.toHaveBeenCalled();
  });

  it("新请求开始后:旧请求晚到的成功不覆盖新状态,旧 finally 不复位新 busy", async () => {
    const calls = mockAiCalls();
    const { translation } = setup();

    const p1 = translation.toggle();
    await waitAiCall(calls, 1);
    // 取消第一轮并立即开始第二轮
    translation.reset();
    const p2 = translation.toggle();
    await waitAiCall(calls, 2);
    expect(translation.translating.value).toBe(true);

    // 第二轮先完成
    calls[1].resolve("新译文");
    await p2;
    expect(translation.translatedText.value).toBe("新译文");

    // 第一轮晚到:结果被丢弃;其 finally 不得清掉第二轮已落定状态
    calls[0].resolve("旧译文");
    await p1;
    expect(translation.translatedText.value).toBe("新译文");
    expect(translation.translating.value).toBe(false);
    expect(toastError).not.toHaveBeenCalled();
  });

  it("旧请求在取消后失败不 toast;当前请求失败才 toast", async () => {
    const calls = mockAiCalls();
    const { translation } = setup();

    const p1 = translation.toggle();
    await waitAiCall(calls, 1);
    translation.reset();
    calls[0].reject(new Error("canceled by user"));
    await p1;
    expect(toastError).not.toHaveBeenCalled();

    const p2 = translation.toggle();
    await waitAiCall(calls, 2);
    calls[1].reject(new Error("network down"));
    await p2;
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(translation.translating.value).toBe(false);
  });

  it("请求期间身份被外部改变(未经 reset):结果同样作废", async () => {
    const calls = mockAiCalls();
    const { translation, currentIdentity } = setup();
    const pending = translation.toggle();
    await waitAiCall(calls, 1);
    currentIdentity.value = "other.md";
    calls[0].resolve("译文");
    await pending;
    expect(translation.translatedText.value).toBeNull();
    expect(translation.showTranslated.value).toBe(false);
  });

  it("原文/译文切换不发起请求;retranslate 跳过缓存覆盖旧译文", async () => {
    const calls = mockAiCalls();
    const { translation } = setup();
    const pending = translation.toggle();
    await waitAiCall(calls, 1);
    calls[0].resolve("译文-v1");
    await pending;

    // 原文/译文切换不发起请求
    await translation.toggle();
    expect(translation.showTranslated.value).toBe(false);
    await translation.toggle();
    expect(translation.showTranslated.value).toBe(true);
    expect(calls.length).toBe(1);

    // 重新翻译:跳过缓存,成功覆盖
    const p2 = translation.retranslate();
    await waitAiCall(calls, 2);
    calls[1].resolve("译文-v2");
    await p2;
    expect(getCachedMock).toHaveBeenCalledTimes(1); // 仅首轮查过缓存
    expect(translation.translatedText.value).toBe("译文-v2");
    expect(translation.showTranslated.value).toBe(true);
  });

  it("无身份或空正文时不发起翻译", async () => {
    const { translation, currentIdentity } = setup();
    currentIdentity.value = null;
    await translation.toggle();
    expect(translation.translating.value).toBe(false);
    expect(getCachedMock).not.toHaveBeenCalled();
  });
});
