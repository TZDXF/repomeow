/**
 * 技能安全扫描结果缓存(IndexedDB,经 `idb` 访问):以「技能 id」为键持久化
 * 最近一次扫描报告,条目内保存技能内容指纹(后端 rl_skill_tokens 的 hash,
 * 覆盖描述与全部文件路径+字节),内容变化即未命中;保留 30 天,让同一技能
 * 在过期前重复打开不再调用 AI(重启应用后仍有效)。
 *
 * 约定与翻译缓存一致:缓存层绝不抛错——IndexedDB 不可用、打开失败、读写
 * 异常一律静默降级为未命中/空操作,绝不影响扫描主流程;调用方无需 try/catch。
 */

import { openDB, type DBSchema, type IDBPDatabase } from "idb";
import type { ResourceSkillScanReport } from "@/lib/resource-library";

export const SCAN_CACHE_DB = "repomeow-scan-cache";
export const SCAN_CACHE_STORE = "scans";
/** 缓存保留时长:30 天(自写入起算,不随读取续期;与翻译缓存一致) */
export const SCAN_CACHE_TTL_MS = 30 * 24 * 60 * 60 * 1000;

interface ScanCacheEntry {
  /** 写入时的技能内容指纹;与当前指纹不符即视为未命中 */
  fingerprint: string;
  report: ResourceSkillScanReport;
  /** 写入时间戳(ms),过期判定与清扫依据 */
  createdAt: number;
}

interface ScanCacheSchema extends DBSchema {
  scans: {
    key: string;
    value: ScanCacheEntry;
    indexes: { createdAt: "createdAt" };
  };
}

type CacheDb = IDBPDatabase<ScanCacheSchema>;

let dbPromise: Promise<CacheDb | null> | null = null;
/** 每个应用会话只在首次打开时全量清扫一次过期条目 */
let swept = false;

function openCacheDb(): Promise<CacheDb | null> {
  if (typeof indexedDB === "undefined") return Promise.resolve(null);
  dbPromise ??= openDB<ScanCacheSchema>(SCAN_CACHE_DB, 1, {
    upgrade(db) {
      const store = db.createObjectStore(SCAN_CACHE_STORE);
      store.createIndex("createdAt", "createdAt");
    },
  }).catch(() => null);
  if (!swept) {
    swept = true;
    void dbPromise.then((db) => (db ? sweepExpired(db) : null));
  }
  return dbPromise;
}

/** 取缓存的扫描报告;未命中/指纹不符/已过期/任何异常返回 null。过期条目顺手删除。 */
export async function getCachedScanReport(
  skillId: string,
  fingerprint: string,
): Promise<ResourceSkillScanReport | null> {
  if (!skillId || !fingerprint) return null;
  try {
    const db = await openCacheDb();
    if (!db) return null;
    const entry = await db.get(SCAN_CACHE_STORE, skillId);
    if (!entry) return null;
    if (entry.fingerprint !== fingerprint) return null;
    if (entry.createdAt + SCAN_CACHE_TTL_MS <= Date.now()) {
      void db.delete(SCAN_CACHE_STORE, skillId);
      return null;
    }
    return entry.report;
  } catch {
    return null;
  }
}

/** 写入缓存的扫描报告;失败静默(fire-and-forget 调用,不影响扫描主流程) */
export async function putCachedScanReport(
  skillId: string,
  fingerprint: string,
  report: ResourceSkillScanReport,
): Promise<void> {
  if (!skillId || !fingerprint) return;
  try {
    const db = await openCacheDb();
    if (!db) return;
    const entry: ScanCacheEntry = { fingerprint, report, createdAt: Date.now() };
    await db.put(SCAN_CACHE_STORE, entry, skillId);
  } catch {
    // 缓存写入失败不影响扫描结果
  }
}

/** 全量清扫过期条目(读取时已有惰性过期,这里兜底清理再也不会被读到的孤儿条目) */
export async function purgeExpiredScans(): Promise<void> {
  try {
    const db = await openCacheDb();
    if (db) await sweepExpired(db);
  } catch {
    // 清扫失败无碍,下次会话再试
  }
}

async function sweepExpired(db: CacheDb): Promise<void> {
  try {
    const tx = db.transaction(SCAN_CACHE_STORE, "readwrite");
    const cutoff = Date.now() - SCAN_CACHE_TTL_MS;
    const expiredKeys = await tx.store
      .index("createdAt")
      .getAllKeys(IDBKeyRange.upperBound(cutoff));
    for (const key of expiredKeys) tx.store.delete(key);
    await tx.done;
  } catch {
    // 同上,清扫失败静默
  }
}
