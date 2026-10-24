import { describe, expect, it } from "vitest";
import { mergeSafePreviewers } from "./markdown-extensions";

// vue-stream-markdown 默认 html 预览沙箱是 allow-scripts(脚本会执行),
// mergeSafePreviewers 必须强制空沙箱且不可被调用方放宽(XSS 评估的修复点)。
describe("mergeSafePreviewers", () => {
  it("未传配置时返回空沙箱 html 预览(保留内置预览启用语义)", () => {
    expect(mergeSafePreviewers()).toEqual({ html: { sandbox: "" } });
    expect(mergeSafePreviewers(true)).toEqual({ html: { sandbox: "" } });
  });

  it("调用方配置浅合并,但 html.sandbox 强制为空不可放宽", () => {
    expect(mergeSafePreviewers({ html: { sandbox: "allow-scripts", height: "200px" } })).toEqual({
      html: { sandbox: "", height: "200px" },
    });
    expect(mergeSafePreviewers({ placement: "center" })).toEqual({
      placement: "center",
      html: { sandbox: "" },
    });
  });

  it("显式 false(关闭预览)原样保留", () => {
    expect(mergeSafePreviewers(false)).toBe(false);
  });
});
