/**
 * AI 指令/规则文件(CLAUDE.md / AGENTS.md 等)正文中的 `@path/to/file.ext`
 * 文件引用解析:在渲染后的 Markdown DOM 里把提及 linkify 成可点击按钮,
 * 点击由外层事件委托统一处理(跳转抽屉预览)。
 *
 * 识别规则(对齐 Claude Code 的 @ 提及习惯,宁缺毋滥):
 * - `@` 前不能是单词字符或 `/`(排除邮箱 user@host、URL 路径段);
 * - 路径段可带前导点(`.cursor/rules`、`.claude/skills` 这类隐藏目录),
 *   其余字符为单词字符/`.`/`-`,可含 `/` 分隔的多级目录;
 * - 末段必须带扩展名(`@user` 这类纯名字不匹配)。
 * 代码块/行内代码/已有链接内的文本不做 linkify(由 DOM 遍历跳过)。
 */

import { toForwardSlash } from "@/lib/path";

/** 匹配 `@docs/guide.md` / `@.claude/skills/foo/SKILL.md` / `@foo.tar.gz` 形态的提及 */
const FILE_REF_RE = /(?<![\w/])@((?:\.?[\w-][\w.-]*\/)*\.?[\w-][\w.-]*\.[\w-]+)/g;

/** 从文本中提取全部 @ 文件引用(供测试与纯文本场景复用) */
export function extractFileRefs(text: string): string[] {
  FILE_REF_RE.lastIndex = 0;
  return [...text.matchAll(FILE_REF_RE)].map((m) => m[1]);
}

/**
 * 把引用解析为仓库内相对路径:相对当前文件所在目录,
 * 归一化 `.`/`..`;越出根目录的 `..` 直接丢弃(越界访问由后端 read_file_preview 兜底拒绝)。
 */
export function resolveRefPath(currentRel: string, ref: string): string {
  const segments = toForwardSlash(currentRel).split("/").slice(0, -1);
  for (const seg of toForwardSlash(ref).split("/")) {
    if (!seg || seg === ".") continue;
    if (seg === "..") segments.pop();
    else segments.push(seg);
  }
  return segments.join("/");
}

/** linkify 产物按钮的统一类名(事件委托经 closest 匹配) */
export const FILE_REF_CLASS = "ai-file-ref";

/**
 * 遍历 container 内文本节点,把 @ 文件引用替换为 <button class="ai-file-ref">。
 * 跳过 A / CODE / PRE / BUTTON 子树(链接、代码、已处理的按钮);
 * 幂等:重复调用时已替换的按钮在跳过名单内,不会二次处理。
 */
export function linkifyFileRefs(container: HTMLElement): void {
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      const text = node.textContent;
      FILE_REF_RE.lastIndex = 0;
      if (!text || !FILE_REF_RE.test(text)) {
        return NodeFilter.FILTER_REJECT;
      }
      const parent = node.parentElement;
      if (!parent || parent.closest("a, code, pre, button")) {
        return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });

  const targets: Text[] = [];
  while (walker.nextNode()) targets.push(walker.currentNode as Text);

  for (const node of targets) {
    const text = node.textContent ?? "";
    FILE_REF_RE.lastIndex = 0;
    const fragment = document.createDocumentFragment();
    let last = 0;
    for (const match of text.matchAll(FILE_REF_RE)) {
      const index = match.index;
      if (index > last) fragment.append(document.createTextNode(text.slice(last, index)));
      const button = document.createElement("button");
      button.type = "button";
      button.className = `${FILE_REF_CLASS} cursor-pointer font-medium text-primary underline decoration-dotted underline-offset-2 hover:decoration-solid`;
      button.dataset.ref = match[1];
      button.title = match[1];
      button.textContent = match[0];
      fragment.append(button);
      last = index + match[0].length;
    }
    if (last < text.length) fragment.append(document.createTextNode(text.slice(last)));
    node.parentNode?.replaceChild(fragment, node);
  }
}
