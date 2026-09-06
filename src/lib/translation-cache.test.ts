/**
 * 翻译缓存单测:node 环境经 fake-indexeddb 提供全局 IndexedDB 实现,
 * 用例间靠互不相同的正文内容隔离,不重建数据库。
 */
import "fake-indexeddb/auto";
import { beforeAll, describe, expect, it } from "vitest";
import { openDB } from "idb";
import {
  TRANSLATION_CACHE_DB,
  TRANSLATION_CACHE_STORE,
  TRANSLATION_CACHE_TTL_MS,
  getCachedTranslation,
  putCachedTranslation,
  purgeExpiredTranslations,
} from "./translation-cache";

/** 把库内译文等于 translatedText 的条目 createdAt 回拨 ageMs(模拟过期写入) */
async function backdate(translatedText: string, ageMs: number) {
  const db = await openDB(TRANSLATION_CACHE_DB, 1);
  const tx = db.transaction(TRANSLATION_CACHE_STORE, "readwrite");
  // getAllKeys 与 getAll 按同一键序返回,按下标对齐取每行的键(键为库外主键,值里没有)
  const keys = await tx.store.getAllKeys();
  const rows = (await tx.store.getAll()) as { translatedText: string }[];
  const createdAt = Date.now() - ageMs;
  rows.forEach((row, i) => {
    if (row.translatedText === translatedText) {
      void tx.store.put({ ...row, createdAt }, keys[i]);
    }
  });
  await tx.done;
  db.close();
}

async function storedTexts(): Promise<string[]> {
  const db = await openDB(TRANSLATION_CACHE_DB, 1);
  const rows = (await db.getAll(TRANSLATION_CACHE_STORE)) as { translatedText: string }[];
  db.close();
  return rows.map((row) => row.translatedText);
}

/** 等待缓存读取中 fire-and-forget 的删除事务提交 */
function flushIndexedDb(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 10));
}

describe("translation-cache", () => {
  beforeAll(async () => {
    // 先经模块 API 打开一次,确保 store 与 createdAt 索引由 upgrade 创建完毕
    await getCachedTranslation("seed", "zh-CN");
  });

  it("写入后按内容 hash 命中缓存", async () => {
    const text = "# Hello\n\nroundtrip content";
    await putCachedTranslation(text, "zh-CN", "# 你好\n\n往返内容");
    expect(await getCachedTranslation(text, "zh-CN")).toBe("# 你好\n\n往返内容");
  });

  it("未缓存的正文返回 null", async () => {
    expect(await getCachedTranslation("never translated content", "zh-CN")).toBeNull();
  });

  it("正文变化即视为未命中", async () => {
    await putCachedTranslation("content v1", "zh-CN", "v1 译文");
    expect(await getCachedTranslation("content v1 (edited)", "zh-CN")).toBeNull();
  });

  it("不同目标语言互相独立", async () => {
    const text = "language isolation content";
    await putCachedTranslation(text, "zh-CN", "中文译文");
    expect(await getCachedTranslation(text, "en-US")).toBeNull();
    expect(await getCachedTranslation(text, "zh-CN")).toBe("中文译文");
  });

  it("超过保留期的条目读取时按未命中处理并清除", async () => {
    const text = "expired content";
    await putCachedTranslation(text, "zh-CN", "过期译文");
    await backdate("过期译文", TRANSLATION_CACHE_TTL_MS + 60_000);
    expect(await getCachedTranslation(text, "zh-CN")).toBeNull();
    await flushIndexedDb();
    expect(await storedTexts()).not.toContain("过期译文");
  });

  it("purgeExpiredTranslations 清扫过期条目、保留新鲜条目", async () => {
    await putCachedTranslation("purge expired a", "zh-CN", "旧译文 A");
    await putCachedTranslation("purge expired b", "en-US", "旧译文 B");
    await putCachedTranslation("purge fresh", "zh-CN", "新鲜译文");
    await backdate("旧译文 A", TRANSLATION_CACHE_TTL_MS + 1);
    await backdate("旧译文 B", TRANSLATION_CACHE_TTL_MS + 1);
    await purgeExpiredTranslations();
    const texts = await storedTexts();
    expect(texts).toContain("新鲜译文");
    expect(texts).not.toContain("旧译文 A");
    expect(texts).not.toContain("旧译文 B");
  });
});
