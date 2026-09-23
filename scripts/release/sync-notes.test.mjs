import assert from "node:assert/strict";
import test from "node:test";
import { syncNotes } from "./sync-notes.mjs";

const manifest = {
  version: "0.2.6",
  notes: "See the assets to download and install this version.",
  pub_date: "2026-09-23T13:16:36.325Z",
  platforms: { "windows-x86_64": { signature: "signature", url: "https://example.com/setup.exe" } },
};

test("同步中文 Markdown 正文，只修改 notes，保留原对象", () => {
  const result = syncNotes(manifest, { tag_name: "v0.2.6", body: "\n### 新增功能\n- 内嵌终端\n" });
  assert.equal(result.notes, "### 新增功能\n- 内嵌终端");
  assert.deepEqual({ ...result, notes: manifest.notes }, manifest);
  assert.notEqual(result, manifest);
});

test("拒绝空说明和默认提示", () => {
  for (const body of [undefined, "", "  ", manifest.notes]) {
    assert.throws(() => syncNotes(manifest, { tag_name: "v0.2.6", body }), /正文为空或仍为占位/);
  }
});

test("拒绝将不同版本的说明写入更新元数据", () => {
  assert.throws(() => syncNotes(manifest, { tag_name: "v0.2.7", body: "更新说明" }), /版本与 Release tag 不一致/);
});
