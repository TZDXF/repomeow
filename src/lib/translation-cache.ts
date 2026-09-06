/**
 * Markdown 翻译缓存(IndexedDB,经 `idb` 访问):以「目标语言 + 正文内容
 * SHA-256」为键持久化译文,保留 30 天,让同一份文件的内容在过期前重复翻译
 * 不再调用 AI(换文件/重启应用后仍有效)。
 *
 * 约定:缓存层绝不抛错——IndexedDB 不可用、打开失败、读写异常一律静默降级
 * 为未命中/空操作,绝不影响翻译主流程;调用方无需 try/catch。
 */

import { openDB, type DBSchema, type IDBPDatabase } from "idb";

export const TRANSLATION_CACHE_DB = "repomeow-cache";
export const TRANSLATION_CACHE_STORE = "translations";
/** 缓存保留时长:30 天(自写入起算,不随读取续期) */
export const TRANSLATION_CACHE_TTL_MS = 30 * 24 * 60 * 60 * 1000;

interface TranslationCacheEntry {
  /** 正文内容 SHA-256(hex),用于排查与未来迁移 */
  hash: string;
  /** 目标语言(zh-CN / en-US) */
  language: string;
  translatedText: string;
  /** 写入时间戳(ms),过期判定与清扫依据 */
  createdAt: number;
}

interface TranslationCacheSchema extends DBSchema {
  translations: {
    key: string;
    value: TranslationCacheEntry;
    indexes: { createdAt: "createdAt" };
  };
}

type CacheDb = IDBPDatabase<TranslationCacheSchema>;

let dbPromise: Promise<CacheDb | null> | null = null;
/** 每个应用会话只在首次打开时全量清扫一次过期条目 */
let swept = false;

function openCacheDb(): Promise<CacheDb | null> {
  if (typeof indexedDB === "undefined") return Promise.resolve(null);
  dbPromise ??= openDB<TranslationCacheSchema>(TRANSLATION_CACHE_DB, 1, {
    upgrade(db) {
      const store = db.createObjectStore(TRANSLATION_CACHE_STORE);
      store.createIndex("createdAt", "createdAt");
    },
  }).catch(() => null);
  if (!swept) {
    swept = true;
    void dbPromise.then((db) => (db ? sweepExpired(db) : null));
  }
  return dbPromise;
}

/** 缓存键:语言与 hash 用 `:` 分隔(两段内都不含冒号) */
async function cacheKey(text: string, language: string): Promise<string> {
  return `${language}:${await hashContent(text)}`;
}

async function hashContent(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** 取缓存的译文;未命中/已过期/任何异常返回 null。过期条目顺手删除。 */
export async function getCachedTranslation(text: string, language: string): Promise<string | null> {
  if (!text) return null;
  try {
    const db = await openCacheDb();
    if (!db) return null;
    const key = await cacheKey(text, language);
    const entry = await db.get(TRANSLATION_CACHE_STORE, key);
    if (!entry) return null;
    if (entry.createdAt + TRANSLATION_CACHE_TTL_MS <= Date.now()) {
      void db.delete(TRANSLATION_CACHE_STORE, key);
      return null;
    }
    return entry.translatedText;
  } catch {
    return null;
  }
}

/** 写入缓存的译文;失败静默(fire-and-forget 调用,不影响翻译主流程) */
export async function putCachedTranslation(
  text: string,
  language: string,
  translatedText: string,
): Promise<void> {
  if (!text || !translatedText) return;
  try {
    const db = await openCacheDb();
    if (!db) return;
    const hash = await hashContent(text);
    const entry: TranslationCacheEntry = {
      hash,
      language,
      translatedText,
      createdAt: Date.now(),
    };
    await db.put(TRANSLATION_CACHE_STORE, entry, `${language}:${hash}`);
  } catch {
    // 缓存写入失败不影响翻译结果
  }
}

/** 全量清扫过期条目(读取时已有惰性过期,这里兜底清理再也不会被读到的孤儿条目) */
export async function purgeExpiredTranslations(): Promise<void> {
  try {
    const db = await openCacheDb();
    if (db) await sweepExpired(db);
  } catch {
    // 清扫失败无碍,下次会话再试
  }
}

async function sweepExpired(db: CacheDb): Promise<void> {
  try {
    const tx = db.transaction(TRANSLATION_CACHE_STORE, "readwrite");
    const cutoff = Date.now() - TRANSLATION_CACHE_TTL_MS;
    const expiredKeys = await tx.store
      .index("createdAt")
      .getAllKeys(IDBKeyRange.upperBound(cutoff));
    for (const key of expiredKeys) tx.store.delete(key);
    await tx.done;
  } catch {
    // 同上,清扫失败静默
  }
}
