import type { WikiOutlinePage } from "@/types";

/**
 * wiki 大纲 XML 容错解析(LLM 输出对接,必须在 node 环境可测,故不用 DOMParser):
 * 1. 剥 markdown fence / 前导噪音,定位 <wiki_structure> 区域;
 * 2. 输出被 token 截断时补合成闭合标签再解析;
 * 3. 页面块一律正则提取(<page id="...">...</page>),未闭合的尾块补 </page> 抢救;
 * 4. 后处理:id slug 化去重、importance 归一、relevantFiles 过滤到真实存在的文件、
 *    按顺序分配页面文件名 `NN-slug.md`。
 */

export interface ParsedWikiOutline {
  title: string;
  description: string;
  pages: WikiOutlinePage[];
}

function unescapeXml(s: string): string {
  return s
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&amp;/g, "&");
}

function tagText(block: string, tag: string): string {
  const m = block.match(new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`));
  return m ? unescapeXml(m[1].trim()) : "";
}

function tagTexts(block: string, tag: string): string[] {
  const re = new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`, "g");
  const out: string[] = [];
  for (const m of block.matchAll(re)) {
    const v = unescapeXml(m[1].trim());
    if (v) out.push(v);
  }
  return out;
}

/** 剥 fence 与前言,截取 <wiki_structure> 区域;截断时补合成闭合标签 */
function extractStructureRegion(raw: string): string {
  let s = raw.replace(/```(?:xml)?/gi, "");
  const start = s.indexOf("<wiki_structure");
  if (start === -1) return "";
  s = s.slice(start);
  if (!s.includes("</wiki_structure>")) {
    // 截断位置可能在某个 page 块中间:补 page/pages/structure 三层闭合
    s = `${s}</page></pages></wiki_structure>`;
  }
  return s;
}

/** 解析 <sections> 分组,返回 pageId → section 标题 */
function parseSections(region: string): Map<string, string> {
  const map = new Map<string, string>();
  const re = /<section\b[^>]*>([\s\S]*?)<\/section>/g;
  for (const m of region.matchAll(re)) {
    const block = m[1];
    const title = tagText(block, "title");
    const pagesTag = tagText(block, "pages");
    for (const id of pagesTag.split(/[\s,]+/).filter(Boolean)) {
      if (title) map.set(id, title);
    }
  }
  return map;
}

/** 标题/描述转 slug:小写字母数字 + 连字符;CJK 字符保留拼音不可行,退回 page-N */
function slugify(text: string, fallback: string): string {
  const slug = text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
  return slug || fallback;
}

/** 归一化 LLM 给的文件路径:去 ./ 与 / 前缀、反斜杠转正斜杠 */
function normalizeFilePath(p: string): string {
  return p.replace(/\\/g, "/").replace(/^\.?\//, "");
}

/**
 * 解析 LLM 输出的大纲 XML。validFiles 提供时,relevantFiles 过滤到该集合
 * (防 LLM 幻觉路径);解析不出任何页面时抛错(调用方走失败重试)
 */
export function parseWikiOutline(raw: string, validFiles?: Set<string>): ParsedWikiOutline {
  const region = extractStructureRegion(raw);
  if (!region) {
    throw new Error("wiki outline: no <wiki_structure> found");
  }

  const sections = parseSections(region);

  // 结构级 title/description:首个 <page>/<section> 之前的部分
  const headEnd = region.search(/<(?:page|section)\b/);
  const head = headEnd === -1 ? region : region.slice(0, headEnd);
  const title = tagText(head, "title");
  const description = tagText(head, "description");

  // 按 <page 切分而非全量正则:某页缺 </page> 闭合时不会吞噬后续页面
  // (<pages> 因 \b 边界不会被误切)
  const fragments = region.split(/<page\b/).slice(1);
  const pages: WikiOutlinePage[] = [];
  const usedIds = new Set<string>();

  for (const frag of fragments) {
    const idMatch = frag.match(/^[^>]*?\bid\s*=\s*"([^"]+)"/);
    if (!idMatch) continue;
    const rawId = idMatch[1].trim();
    // 截掉闭合标签及其后内容(若存在);未闭合时整段都是该页字段
    const block = frag.replace(/<\/page>[\s\S]*$/, "");
    const pageTitle = tagText(block, "title") || rawId;
    // id 以 LLM 输出为准做 slug;重复时加序号后缀
    let id = slugify(rawId, `page-${pages.length + 1}`);
    for (let n = 2; usedIds.has(id); n++) {
      id = `${slugify(rawId, `page-${pages.length + 1}`)}-${n}`;
    }
    usedIds.add(id);

    const importanceRaw = tagText(block, "importance").toLowerCase();
    const importance = ["high", "medium", "low"].includes(importanceRaw) ? importanceRaw : "medium";

    let relevantFiles = tagTexts(block, "file_path").map(normalizeFilePath);
    if (validFiles) {
      relevantFiles = relevantFiles.filter((f) => validFiles.has(f));
    }

    pages.push({
      id,
      file: `${String(pages.length + 1).padStart(2, "0")}-${id}.md`,
      title: pageTitle,
      description: tagText(block, "description"),
      section: sections.get(rawId) ?? sections.get(id) ?? null,
      importance,
      relevantFiles: [...new Set(relevantFiles)],
      relatedPages: tagTexts(block, "related"),
    });
  }

  if (!pages.length) {
    throw new Error("wiki outline: no <page> parsed");
  }
  return { title, description, pages };
}
