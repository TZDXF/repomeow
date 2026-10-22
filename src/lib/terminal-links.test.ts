import { beforeEach, describe, expect, it, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "vue-sonner";
import { activateTerminalLink } from "./terminal-links";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("vue-sonner", () => ({ toast: { error: vi.fn() } }));

function click(ctrlKey = true, button = 0) {
  return { ctrlKey, button, preventDefault: vi.fn() } as unknown as MouseEvent;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(openUrl).mockResolvedValue();
});

describe("activateTerminalLink", () => {
  it.each(["http://localhost:1420/", "https://example.com/path?q=hello#section"])(
    "Ctrl + 左键使用系统浏览器打开 %s",
    (url) => {
      const event = click();
      activateTerminalLink(event, url);
      expect(openUrl).toHaveBeenCalledWith(url);
      expect(event.preventDefault).toHaveBeenCalled();
    },
  );

  it("普通点击和非左键不打开链接", () => {
    activateTerminalLink(click(false), "https://example.com");
    activateTerminalLink(click(true, 1), "https://example.com");
    activateTerminalLink(click(true, 2), "https://example.com");
    expect(openUrl).not.toHaveBeenCalled();
  });

  it.each(["javascript:alert(1)", "file:///C:/test.txt", "mailto:a@example.com", "invalid"])(
    "拒绝非网页或无效地址 %s",
    (url) => {
      activateTerminalLink(click(), url);
      expect(openUrl).not.toHaveBeenCalled();
    },
  );

  it("打开失败时显示错误", async () => {
    vi.mocked(openUrl).mockRejectedValue(new Error("open failed"));
    activateTerminalLink(click(), "https://example.com");
    await Promise.resolve();
    expect(toast.error).toHaveBeenCalledWith("Error: open failed");
  });
});
