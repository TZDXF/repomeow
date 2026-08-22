import { i18n } from "@/i18n";

/** Unix 秒时间戳 → 当前语言的相对时间 */
export function formatRelativeTime(tsSeconds: number | null): string {
  if (!tsSeconds) return i18n.global.t("common.never");
  const diff = Date.now() / 1000 - tsSeconds;
  if (diff < 60) return i18n.global.t("common.justNow");
  if (diff < 3600) return i18n.global.t("common.minutesAgo", { count: Math.floor(diff / 60) });
  if (diff < 86400) return i18n.global.t("common.hoursAgo", { count: Math.floor(diff / 3600) });
  return i18n.global.t("common.daysAgo", { count: Math.floor(diff / 86400) });
}

/** Date → 本地 "YYYY-MM-DD"(与后端报告日期、git log 日期串一致) */
export function formatDate(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

/** "YYYY-MM-DD" → 本地当天 00:00 的 Date(避免 new Date(str) 的 UTC 解析时区偏移) */
export function parseDateStr(s: string): Date {
  const [y, m, d] = s.split("-").map(Number);
  return new Date(y, m - 1, d);
}

/** Unix 秒时间戳 → 当前语言的本地日期时间("YYYY/MM/DD HH:mm") */
export function formatLocalDateTime(tsSeconds: number): string {
  return new Date(tsSeconds * 1000).toLocaleString(i18n.global.locale.value, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** ISO 时间串 → 当前语言的本地日期(仅日期部分);空串/解析失败回退空串 */
export function formatIsoDate(iso: string): string {
  if (!iso) return "";
  const ts = new Date(iso).getTime();
  return Number.isNaN(ts) ? "" : new Date(ts).toLocaleDateString(i18n.global.locale.value);
}

/** 本地时间串 "YYYY-MM-DD HH:MM"(git_log 返回格式)→ 相对时间;超过 30 天或解析失败回退原串 */
export function formatCommitTime(dateStr: string): string {
  // 补 T 使其按 ISO 本地时间解析("YYYY-MM-DD HH:MM" 在部分引擎会按 UTC 或解析失败)
  const ts = new Date(dateStr.replace(" ", "T")).getTime();
  if (Number.isNaN(ts)) return dateStr;
  if (Date.now() - ts >= 30 * 86400_000) return dateStr;
  return formatRelativeTime(Math.floor(ts / 1000));
}
