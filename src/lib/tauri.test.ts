import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { translateCommandErrorForTest, cmd } from "@/lib/tauri";

// ── 任务描述 ─────────────────────────────────────────────────────────────
// 覆盖 src/lib/tauri.ts 在新错误契约下的两个关键路径:
//
// 1. translateCommandError / translateCommandErrorForTest 只识别两种形态:
//      a) JS Error 实例(任意外部错误/被 throws 自己 new Error 包过的)
//      b) 结构化后端错误 { code: string?, message: string } —— code 命中 i18n
//         时返回本地化文案,否则回退到 message,空 message 走 "未知错误" 兜底。
//    不再有"纯字符串"作为合法 AppError 形态的兼容路径(后端不再 wire-compat
//    serialize_str);但 Error.message 仍然要可读。
// 2. cmd<T>() 包装 invoke,在 invoke 抛错时把 Error.message 设为本地化文案,
//    并把原始 rejection 挂在 Error.cause 上,便于上层调试。
//
// Tauri invoke/listen 在 Node 测试环境不可用,通过 vi.hoisted + vi.mock 替换。

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

describe("translateCommandError / cmd 本地化路径(新契约)", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });
  afterEach(() => {
    vi.clearAllMocks();
  });

  describe("translateCommandErrorForTest", () => {
    it("Error 实例:返回 message,避免 'Error: ...' 退化", () => {
      const e = new Error("底层错误");
      expect(translateCommandErrorForTest(e)).toBe("底层错误");
    });

    it("Error.message 为空时:回退到 String(error),仍非空字符串", () => {
      const e = new Error("");
      const out = translateCommandErrorForTest(e);
      // 空 message 时实现走 `error.message || String(error)`,此处不应抛,
      // 也不应返回字面量 "undefined"
      expect(typeof out).toBe("string");
      expect(out.length).toBeGreaterThan(0);
    });

    it("已知错误码 (project_not_found):命中 i18n,返回中文文案,不暴露 message", () => {
      // message 是 Rust 端可能附带的技术细节;用户应看到友好文案
      const serialized = {
        code: "project_not_found",
        message: "project 42",
      };
      expect(translateCommandErrorForTest(serialized)).toBe("项目不存在");
    });

    it("已知错误码 (ai_not_configured):命中 i18n,返回对应提示", () => {
      const serialized = {
        code: "ai_not_configured",
        message: "AI 未配置",
      };
      expect(translateCommandErrorForTest(serialized)).toBe("请先在设置页配置 AI 的 API Key");
    });

    it("未知错误码:回退到 message,不让用户看到英文 code key", () => {
      const serialized = {
        code: "some_unknown_code",
        message: "真实的错误信息",
      };
      expect(translateCommandErrorForTest(serialized)).toBe("真实的错误信息");
    });

    it("无 code,只有 message:返回 message,绝不返回 '[object Object]'", () => {
      const serialized = { message: "原始错误" };
      expect(translateCommandErrorForTest(serialized)).toBe("原始错误");
    });

    it("code 是空字符串:按无 code 处理,回退到 message", () => {
      const serialized = { code: "", message: "降级文案" };
      expect(translateCommandErrorForTest(serialized)).toBe("降级文案");
    });

    it('message 是空字符串,code 未命中:返回 code 本身作为兜底信息', () => {
      // 新契约:有 code 时至少暴露 code 让用户/日志能看到失败身份;
      // 完全空对象/字段类型异常时才返回 "未知错误"
      const serialized = { code: "unknown", message: "" };
      expect(translateCommandErrorForTest(serialized)).toBe("unknown");
    });

    it("对象但字段类型异常 (code/message 不是 string):不崩,走兜底", () => {
      // 模拟序列化层出现意外形态:code/message 都是数字
      const weird = { code: 123, message: 456 };
      expect(translateCommandErrorForTest(weird)).toBe("未知错误");
    });

    it("嵌套对象:不递归处理,只读顶层 code/message", () => {
      const nested = { code: "project_not_found", message: { detail: "x" } };
      // message 不是 string → 走兜底或无 message 路径,但绝不抛
      const out = translateCommandErrorForTest(nested);
      expect(typeof out).toBe("string");
      expect(out.length).toBeGreaterThan(0);
    });

    it("纯字符串输入不再被当作合法 AppError 形态:走兜底 '未知错误'", () => {
      // 新契约:后端 AppError 不再 wire-compat 输出纯字符串;
      // 任何穿过 cmd 的纯字符串 rejection 都视作不可识别,统一走兜底,
      // 避免老调用方依赖 String(err) 取文案的行为被默默保留。
      expect(translateCommandErrorForTest("网络异常")).toBe("未知错误");
    });

    it("null / undefined:走兜底 '未知错误'(稳定可读,不再 String() 暴露 'null')", () => {
      expect(translateCommandErrorForTest(null)).toBe("未知错误");
      expect(translateCommandErrorForTest(undefined)).toBe("未知错误");
    });

    it("数字 / 布尔等基类型:返回 '未知错误',不暴露 '[object Object]'", () => {
      expect(translateCommandErrorForTest(42)).toBe("未知错误");
      expect(translateCommandErrorForTest(true)).toBe("未知错误");
    });
  });

  describe("cmd<T>() 错误包装", () => {
    it("invoke 成功:直接返回 typed payload,不包装", async () => {
      invokeMock.mockResolvedValueOnce({ id: 1, name: "x" });
      const r = await cmd<{ id: number; name: string }>("get_thing");
      expect(r).toEqual({ id: 1, name: "x" });
    });

    it("invoke 拒绝一个序列化错误:抛出 Error.message = i18n 文案,保留 cause", async () => {
      invokeMock.mockRejectedValueOnce({
        code: "schedule_not_found",
        message: "定时任务不存在",
      });

      let caught: unknown;
      try {
        await cmd("list_report_schedules");
      } catch (e) {
        caught = e;
      }
      expect(caught).toBeInstanceOf(Error);
      const err = caught as Error & { cause?: unknown };
      expect(err.message).toBe("定时任务不存在");
      // 原始 rejection 挂在 cause 上,便于上层调试拿到底层 payload
      expect(err.cause).toEqual({
        code: "schedule_not_found",
        message: "定时任务不存在",
      });
    });

    it("invoke 拒绝一个普通 Error 实例:抛出 Error,message 透传,cause 保留", async () => {
      // 新契约允许 Error(因为发起方可能包装抛出);保留 message + cause,
      // 但不再有 "原样返回字符串" 的兼容路径。
      const original = new Error("底层 IO 失败");
      invokeMock.mockRejectedValueOnce(original);

      let caught: unknown;
      try {
        await cmd("noop");
      } catch (e) {
        caught = e;
      }
      expect(caught).toBeInstanceOf(Error);
      const err = caught as Error & { cause?: unknown };
      expect(err.message).toBe("底层 IO 失败");
      expect(err.cause).toBe(original);
    });

    it("参数透传:invoked name + camelCase args 原样下传", async () => {
      invokeMock.mockResolvedValueOnce(null);
      await cmd("save_report_history", {
        projectIds: [1, 2],
        dateFrom: "2026-07-01",
      });
      expect(invokeMock).toHaveBeenCalledWith("save_report_history", {
        projectIds: [1, 2],
        dateFrom: "2026-07-01",
      });
    });
  });
});
