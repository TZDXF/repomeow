import { describe, expect, it } from "vitest";
import { joinPath, splitDirName } from "./path";

describe("joinPath", () => {
  it("跟随父路径的分隔符风格", () => {
    expect(joinPath("D:\\code", "proj")).toBe("D:\\code\\proj");
    expect(joinPath("D:/code", "proj")).toBe("D:/code/proj");
    expect(joinPath("/home/user", "proj")).toBe("/home/user/proj");
  });

  it("去掉父路径的尾随分隔符", () => {
    expect(joinPath("D:\\code\\", "proj")).toBe("D:\\code\\proj");
    expect(joinPath("D:/code/", "proj")).toBe("D:/code/proj");
    expect(joinPath("D:\\code//", "proj")).toBe("D:\\code\\proj");
  });

  it("父路径为根时不再补分隔符", () => {
    expect(joinPath("C:\\", "proj")).toBe("C:\\proj");
    expect(joinPath("C:", "proj")).toBe("C:\\proj");
    expect(joinPath("/", "proj")).toBe("/proj");
  });

  it("忽略父路径首尾空白", () => {
    expect(joinPath("  D:\\code  ", "proj")).toBe("D:\\code\\proj");
  });
});

describe("splitDirName", () => {
  it("保留原路径的分隔符风格", () => {
    expect(splitDirName("D:\\code\\proj")).toEqual({ parent: "D:\\code", name: "proj" });
    expect(splitDirName("D:/code/proj")).toEqual({ parent: "D:/code", name: "proj" });
  });

  it("单段输入的父目录为根分隔符", () => {
    expect(splitDirName("proj")).toEqual({ parent: "/", name: "proj" });
    expect(splitDirName("D:\\")).toEqual({ parent: "\\", name: "D:" });
  });
});
