const OPENING_FRONTMATTER = /^(?:\uFEFF)?---[ \t]*\r?\n/,
  CLOSING_FRONTMATTER = /^(?:---|\.\.\.)[ \t]*(?:\r?\n|$)/m;

/**
 * 将文档头部的 YAML frontmatter 转成可见的代码块。
 * markmend 会把 frontmatter 解析为独立元数据而非渲染节点;仅改写渲染输入,
 * 文件读取、源码视图与翻译所持有的原始内容均不受影响。
 */
export function withVisibleMarkdownFrontmatter(content: string): string {
  const opening = OPENING_FRONTMATTER.exec(content);
  if (!opening) {
    return content;
  }
  const rest = content.slice(opening[0].length),
    closing = CLOSING_FRONTMATTER.exec(rest);
  if (!closing) {
    return content;
  }
  const bom = content.startsWith("\uFEFF") ? "\uFEFF" : "",
    yaml = rest.slice(0, closing.index),
    body = rest.slice(closing.index + closing[0].length);
  return `${bom}\`\`\`yaml\n${yaml}\`\`\`\n${body}`;
}
