/** 提交统计(git_project_stats)的前端聚合与图表数据构建 */
import { formatDate } from "@/lib/format";
import type { GitDayStat, GitFileTypeStat } from "@/types";

/** 提交日历热力图周列数:53 周 ≈ 最近 12 个月(GitHub 贡献图一整年) */
export const COMMIT_CALENDAR_WEEKS = 53;

/** 提交趋势/变更趋势展示的周数(取最近一年) */
export const TREND_WEEKS = 52;

export interface CommitCalendarCell {
  /** 本地日期 YYYY-MM-DD */
  day: string;
  count: number;
  /** 0 = 无提交,1-4 = 按窗口内最大日提交数分档 */
  level: 0 | 1 | 2 | 3 | 4;
  /** 最后一列(本周)里超过今天的格子,渲染为占位 */
  future: boolean;
}

export interface CommitCalendar {
  /** weeks[col][row]:col = 周列,row = 0..6(周一..周日) */
  weeks: CommitCalendarCell[][];
  /** 月份标签:该列的某一行是当月 1 号(标签画在该列上方),month 为 1-12 */
  monthLabels: { col: number; month: number }[];
}

/** 一周的提交聚合;day 为周一日期 YYYY-MM-DD */
export interface GitWeekStat {
  day: string;
  count: number;
  additions: number;
  deletions: number;
}

/**
 * 后端 byDay 的 t → 日期串 "YYYY-MM-DD"。
 * t 的 UTC 日期即提交者本地日期(仅作日历标识),故用 UTC 字段还原,避免浏览者时区串日
 */
export function dayKeyOf(t: number): string {
  const d = new Date(t * 1000);
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");
  return `${d.getUTCFullYear()}-${m}-${day}`;
}

/** 提交数 → 强度档(0-4),阈值与设置页用量热力图一致(25%/50%/75% 分档) */
export function heatLevel(count: number, max: number): 0 | 1 | 2 | 3 | 4 {
  if (count <= 0) return 0;
  const ratio = count / Math.max(max, 1);
  if (ratio >= 0.75) return 4;
  if (ratio >= 0.5) return 3;
  if (ratio >= 0.25) return 2;
  return 1;
}

/**
 * 由按日聚合构建提交日历热力图网格(周列 × 周一~周日行)。
 * 网格从「当前周往前数 WEEKS-1 周的周一」开始,到本周日结束;today 之后的格子标记为 future。
 * 强度分档基于窗口内的最大日提交数(窗口外数据不参与归一化)。
 */
export function buildCommitCalendar(byDay: GitDayStat[], today: Date = new Date()): CommitCalendar {
  const countByDay = new Map(byDay.map((d) => [dayKeyOf(d.t), d.count]));
  const end = startOfDay(today);
  const start = addDays(mondayOfWeek(end), -7 * (COMMIT_CALENDAR_WEEKS - 1));

  let maxCount = 0;
  for (let i = 0; ; i++) {
    const d = addDays(start, i);
    if (d > end) break;
    const count = countByDay.get(formatDate(d)) ?? 0;
    if (count > maxCount) maxCount = count;
  }

  const weeks: CommitCalendarCell[][] = [];
  const monthLabels: { col: number; month: number }[] = [];
  for (let col = 0; col < COMMIT_CALENDAR_WEEKS; col++) {
    const week: CommitCalendarCell[] = [];
    for (let row = 0; row < 7; row++) {
      const d = addDays(start, col * 7 + row);
      const count = countByDay.get(formatDate(d)) ?? 0;
      week.push({
        day: formatDate(d),
        count,
        level: heatLevel(count, maxCount),
        future: d > end,
      });
      if (d.getDate() === 1 && d <= end) {
        monthLabels.push({ col, month: d.getMonth() + 1 });
      }
    }
    weeks.push(week);
  }
  return { weeks, monthLabels };
}

/** 按日聚合 → 按周(周一起)聚合,按周升序;取最近 limit 周(默认一年) */
export function aggregateWeeks(byDay: GitDayStat[], limit = TREND_WEEKS): GitWeekStat[] {
  const weeks = new Map<string, GitWeekStat>();
  for (const d of byDay) {
    const date = new Date(d.t * 1000);
    // 提交者本地日期存在 UTC 刻度上,故用 UTC 字段推算周一
    const weekday = (date.getUTCDay() + 6) % 7;
    const monday = new Date(
      Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate() - weekday),
    );
    const key = dayKeyOf(monday.getTime() / 1000);
    let week = weeks.get(key);
    if (!week) {
      week = { day: key, count: 0, additions: 0, deletions: 0 };
      weeks.set(key, week);
    }
    week.count += d.count;
    week.additions += d.additions;
    week.deletions += d.deletions;
  }
  return [...weeks.values()].sort((a, b) => a.day.localeCompare(b.day)).slice(-limit);
}

/** 累计提交曲线的一点:t 为该日 UTC 零点刻度(秒),total 为截至当日的累计提交数 */
export interface CumulativePoint {
  t: number;
  /** 日期 YYYY-MM-DD(tooltip 展示用) */
  day: string;
  total: number;
}

/**
 * 按日聚合 → 全历史累计提交曲线,按日升序逐点累加。
 * 依赖 byDay 契约(全历史、按日升序、只含有提交的日子);不做时间窗口截断,
 * 曲线覆盖项目从首次提交至今的完整增长走势
 */
export function buildCumulativeCommits(byDay: GitDayStat[]): CumulativePoint[] {
  let total = 0;
  return byDay.map((d) => {
    total += d.count;
    return { t: d.t, day: dayKeyOf(d.t), total };
  });
}

/** Top N + 其余归并为一项(merge 由调用方求和);不足 N 项时原样返回 */
export function topWithOther<T>(items: T[], limit: number, merge: (rest: T[]) => T): T[] {
  if (items.length <= limit) return items;
  return [...items.slice(0, limit), merge(items.slice(limit))];
}

/**
 * 语言类文件扩展名(小写,不含点),对标 GitHub 语言条的 Linguist 口径:
 * 仅编程/脚本/查询/标记语言;图片、字体、音视频、压缩包、二进制产物与
 * 配置/数据文件(json/yaml/toml/xml/lock/csv 等)一律不计
 */
const LANGUAGE_EXTS: ReadonlySet<string> = new Set([
  // 主流编程语言
  "rs", "go", "py", "pyw", "pyi", "java", "kt", "kts", "scala", "groovy", "gradle",
  "c", "h", "cc", "cpp", "cxx", "c++", "hpp", "hh", "hxx", "inl", "cu", "cuh",
  "cs", "csx", "vb", "vbs", "fs", "fsi", "fsx", "m", "mm", "swift",
  "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "coffee",
  "vue", "svelte", "astro", "rb", "erb", "php", "phtml", "dart", "lua",
  "pl", "pm", "r", "jl", "ex", "exs", "erl", "hrl", "clj", "cljs", "cljc",
  "hs", "lhs", "ml", "mli", "nim", "zig", "d", "pas", "f", "f90", "f95", "f03",
  "cob", "cbl", "asm", "s", "v", "sv", "vhd", "vhdl", "sol", "move", "wat",
  "elm", "purs", "rkt", "scm", "lisp", "lsp", "el", "hx", "vala", "cr",
  "odin", "gleam", "mojo", "adb", "ads", "awk", "tcl", "applescript", "vim",
  // 脚本 / Shell / 构建
  "sh", "bash", "zsh", "fish", "ksh", "bat", "cmd", "ps1", "psm1", "psd1", "cmake",
  // 查询 / 接口描述 / 基础设施即代码
  "sql", "graphql", "gql", "proto", "tf", "hcl", "nix",
  // 着色器
  "glsl", "vert", "frag", "geom", "comp", "wgsl", "hlsl",
  // 笔记本 / 排版
  "ipynb", "tex", "sty", "cls", "bib", "rmd",
  // 标记 / 模板 / 样式(GitHub 语言条同样计入 HTML/CSS/Markdown)
  "html", "htm", "xhtml", "css", "scss", "sass", "less", "styl",
  "md", "markdown", "mdx", "rst", "adoc", "asciidoc", "org",
  "ejs", "hbs", "handlebars", "mustache", "twig", "njk", "liquid", "pug", "jade",
  // 无扩展名的脚本类清单文件(与后端 SPECIAL_FILENAMES 对应)
  "dockerfile", "makefile", "jenkinsfile", "vagrantfile", "gemfile", "rakefile",
  "podfile", "justfile",
]);

/**
 * 文件类型分布 → 仅保留语言类(对标 GitHub 语言条),
 * 未知扩展名与后端的 "(other)" 归并键一并排除;保持后端返回的字节降序
 */
export function filterLanguageFileTypes(fileTypes: GitFileTypeStat[]): GitFileTypeStat[] {
  return fileTypes.filter((f) => LANGUAGE_EXTS.has(f.ext));
}

/** 作息热力图取值:weekdayHour 为 7*24 行主序(行 = 周一..周日) */
export function weekdayHourAt(weekdayHour: number[], weekday: number, hour: number): number {
  return weekdayHour[weekday * 24 + hour] ?? 0;
}

function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

/** 所在周的周一(本地时区) */
function mondayOfWeek(d: Date): Date {
  // getDay: 0=周日..6=周六 → 距周一的天数
  return addDays(d, -((d.getDay() + 6) % 7));
}
