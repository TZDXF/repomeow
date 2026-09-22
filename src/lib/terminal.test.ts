import { describe, expect, it } from "vitest";
import { appendCapped, TERMINAL_MAX_OUTPUT_CHARS } from "@/lib/terminal";

describe("appendCapped", () => {
  it("直接拼接未超限的输出", () => {
    expect(appendCapped("abc", "def")).toBe("abcdef");
    expect(appendCapped("", "x")).toBe("x");
  });

  it("超过上限时丢弃最旧部分", () => {
    expect(appendCapped("abcdef", "gh", 6)).toBe("cdefgh");
  });

  it("单块即超限时仅保留尾部", () => {
    expect(appendCapped("", "abcdefgh", 4)).toBe("efgh");
  });

  it("默认上限与后端 MAX_OUTPUT_CHARS 一致", () => {
    const chunk = "x".repeat(TERMINAL_MAX_OUTPUT_CHARS);
    expect(appendCapped(chunk, "y")).toHaveLength(TERMINAL_MAX_OUTPUT_CHARS);
  });
});
