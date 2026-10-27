import { beforeEach, describe, expect, it, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";
import { interceptMarkdownLink } from "./markdown-links";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));

function click(href: string | null, options: Partial<MouseEvent> = {}) {
  const event = {
    target: { closest: () => ({ getAttribute: () => href }) },
    type: "click",
    button: 0,
    preventDefault: vi.fn(),
    stopPropagation: vi.fn(),
    ...options,
  } as unknown as MouseEvent;
  interceptMarkdownLink(event);
  return event;
}

beforeEach(() => vi.clearAllMocks());
describe("Markdown 链接统一拦截", () => {
  it("外链拦截并交给系统打开,不重复冒泡", () => {
    const e = click("https://example.com");
    expect(e.preventDefault).toHaveBeenCalled();
    expect(e.stopPropagation).toHaveBeenCalled();
    expect(openUrl).toHaveBeenCalledWith("https://example.com");
  });
  it("相对路径和锚点不导航,保留场景处理", () => {
    for (const href of ["../a.md", "#heading", "//host/path"]) {
      const e = click(href);
      expect(e.preventDefault).toHaveBeenCalled();
      expect(e.stopPropagation).not.toHaveBeenCalled();
    }
    expect(openUrl).not.toHaveBeenCalled();
  });
  it("危险协议及中键、右键、修饰键均无法触发导航", () => {
    for (const e of [
      click("javascript:alert(1)"),
      click("https://example.com", { type: "auxclick", button: 1 }),
      click("https://example.com", { type: "contextmenu", button: 2 }),
      click("https://example.com", { ctrlKey: true }),
    ]) {
      expect(e.preventDefault).toHaveBeenCalled();
      expect(e.stopPropagation).toHaveBeenCalled();
    }
    expect(openUrl).not.toHaveBeenCalled();
  });
});
