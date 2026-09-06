/**
 * 扫描缓存单测:node 环境经 fake-indexeddb 提供全局 IndexedDB 实现,
 * 用例间靠互不相同的技能 id 隔离,不重建数据库。
 */
import "fake-indexeddb/auto";
import { beforeAll, describe, expect, it } from "vitest";
import { openDB } from "idb";
import {
  SCAN_CACHE_DB,
  SCAN_CACHE_STORE,
  SCAN_CACHE_TTL_MS,
  getCachedScanReport,
  putCachedScanReport,
  purgeExpiredScans,
} from "./scan-cache";
import type { ResourceSkillScanReport } from "./resource-library";

function report(skillId: string, score: number): ResourceSkillScanReport {
  return {
    skillId,
    score,
    level: "low",
    findings: [],
    staticCount: 0,
    suppressedCount: 0,
    filesScanned: 1,
    llmStatus: "ok",
    llmErrorCode: null,
    llmErrorMessage: null,
    llmSummary: null,
    scannedAt: 1_700_000_000,
  };
}

/** 把库内报告分值等于 score 的条目 createdAt 回拨 ageMs(模拟过期写入) */
async function backdate(score: number, ageMs: number) {
  const db = await openDB(SCAN_CACHE_DB, 1);
  const tx = db.transaction(SCAN_CACHE_STORE, "readwrite");
  const keys = await tx.store.getAllKeys();
  const rows = (await tx.store.getAll()) as { report: { score: number } }[];
  const createdAt = Date.now() - ageMs;
  rows.forEach((row, i) => {
    if (row.report.score === score) {
      void tx.store.put({ ...row, createdAt }, keys[i]);
    }
  });
  await tx.done;
  db.close();
}

async function storedReports(): Promise<number[]> {
  const db = await openDB(SCAN_CACHE_DB, 1);
  const rows = (await db.getAll(SCAN_CACHE_STORE)) as { report: { score: number } }[];
  db.close();
  return rows.map((row) => row.report.score);
}

/** 等待缓存读取中 fire-and-forget 的删除事务提交 */
function flushIndexedDb(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 10));
}

describe("scan-cache", () => {
  beforeAll(async () => {
    // 先经模块 API 打开一次,确保 store 与 createdAt 索引由 upgrade 创建完毕
    await getCachedScanReport("seed", "fp");
  });

  it("写入后按技能 id 命中缓存", async () => {
    await putCachedScanReport("skill-a", "fp-1", report("skill-a", 10));
    const hit = await getCachedScanReport("skill-a", "fp-1");
    expect(hit?.score).toBe(10);
    expect(hit?.scannedAt).toBe(1_700_000_000);
  });

  it("未缓存的技能返回 null", async () => {
    expect(await getCachedScanReport("skill-never", "fp")).toBeNull();
  });

  it("技能内容指纹变化即视为未命中", async () => {
    await putCachedScanReport("skill-b", "fp-old", report("skill-b", 20));
    expect(await getCachedScanReport("skill-b", "fp-new")).toBeNull();
    // 原指纹仍命中
    const hit = await getCachedScanReport("skill-b", "fp-old");
    expect(hit?.score).toBe(20);
  });

  it("超过保留期的条目读取时按未命中处理并清除", async () => {
    await putCachedScanReport("skill-c", "fp", report("skill-c", 30));
    await backdate(30, SCAN_CACHE_TTL_MS + 60_000);
    expect(await getCachedScanReport("skill-c", "fp")).toBeNull();
    await flushIndexedDb();
    expect(await storedReports()).not.toContain(30);
  });

  it("purgeExpiredScans 清扫过期条目、保留新鲜条目", async () => {
    await putCachedScanReport("skill-d", "fp", report("skill-d", 40));
    await putCachedScanReport("skill-e", "fp", report("skill-e", 50));
    await backdate(40, SCAN_CACHE_TTL_MS + 1);
    await purgeExpiredScans();
    const scores = await storedReports();
    expect(scores).toContain(50);
    expect(scores).not.toContain(40);
  });
});
