import { readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

const MAX_ENTRY_BYTES = 500 * 1024;
const distDir = resolve(import.meta.dirname, "../dist");
const html = readFileSync(resolve(distDir, "index.html"), "utf8");
const entrySrc = html.match(/<script\b[^>]*\btype="module"[^>]*\bsrc="([^"]+)"/i)?.[1];

if (!entrySrc) {
  throw new Error("未在 dist/index.html 找到首屏 module 入口");
}

const entryPath = resolve(distDir, entrySrc.replace(/^\/+/, ""));
const size = statSync(entryPath).size;
if (size > MAX_ENTRY_BYTES) {
  throw new Error(
    `首屏入口 ${entrySrc} 为 ${(size / 1024).toFixed(1)} KiB，超过 500 KiB 预算；请按路由或重型功能拆包`,
  );
}

console.log(`首屏入口体积检查通过：${entrySrc}，${(size / 1024).toFixed(1)} KiB / 500 KiB`);
