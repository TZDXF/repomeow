import { describe, expect, it } from "vitest";
import { chatSidebarBounds, clampChatSidebarWidth } from "@/lib/chat-layout";

describe("项目问答右侧栏宽度", () => {
  it("普通窗口为主工作区保留 360px,并限制拖拽宽度", () => {
    expect(chatSidebarBounds(1280)).toEqual({ min: 420, max: 920 });
    expect(clampChatSidebarWidth(200, 1280)).toBe(420);
    expect(clampChatSidebarWidth(500, 1280)).toBe(500);
    expect(clampChatSidebarWidth(1200, 1280)).toBe(920);
  });

  it("窄窗口下侧栏最多占 60%,不会被最小宽度撑出容器", () => {
    expect(chatSidebarBounds(600)).toEqual({ min: 360, max: 360 });
    expect(clampChatSidebarWidth(720, 600)).toBe(360);
    expect(clampChatSidebarWidth(420, 320)).toBe(192);
  });

  it("页面隐藏或容器尚无尺寸时不产生负宽度", () => {
    expect(chatSidebarBounds(0)).toEqual({ min: 0, max: 0 });
    expect(clampChatSidebarWidth(500, 0)).toBe(0);
  });
});
