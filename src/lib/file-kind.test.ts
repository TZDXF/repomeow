import { describe, expect, it } from "vitest";
import { isReadmeName } from "./file-kind";

describe("isReadmeName", () => {
  it("识别常见大小写与扩展名变体", () => {
    expect(isReadmeName("README.md")).toBe(true);
    expect(isReadmeName("readme.md")).toBe(true);
    expect(isReadmeName("Readme.MD")).toBe(true);
    expect(isReadmeName("README.markdown")).toBe(true);
    expect(isReadmeName("README.txt")).toBe(true);
    expect(isReadmeName("README")).toBe(true);
  });

  it("不把非 README 文件误判", () => {
    expect(isReadmeName("README.rst")).toBe(false);
    expect(isReadmeName("README.md.bak")).toBe(false);
    expect(isReadmeName("readme-legacy.md")).toBe(false);
    expect(isReadmeName("myreadme.md")).toBe(false);
    expect(isReadmeName("CONTRIBUTING.md")).toBe(false);
  });
});
