/** 图片扩展名集合(文件预览与提交图片 diff 预览共用的口径) */
export const IMAGE_EXTS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "svg",
  "ico",
  "bmp",
  "avif",
]);

/** 取路径扩展名(小写、不含点;只看最后一个路径段内的最后一个点) */
export function extOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/** 路径是否按图片处理(按扩展名判定) */
export function isImagePath(path: string): boolean {
  return IMAGE_EXTS.has(extOf(path));
}

/** README 文件名识别(.md/.markdown/.txt/无扩展名,大小写不敏感,覆盖常见大小写变体) */
export function isReadmeName(name: string): boolean {
  return /^readme(?:\.(?:md|markdown|txt))?$/i.test(name);
}

/** 扩展名 → data URL 的 mime 类型;未知扩展回退八进制流 */
export function imageMimeOf(path: string): string {
  switch (extOf(path)) {
    case "png":
      return "image/png";
    case "jpg":
    case "jpeg":
      return "image/jpeg";
    case "gif":
      return "image/gif";
    case "webp":
      return "image/webp";
    case "svg":
      return "image/svg+xml";
    case "ico":
      return "image/x-icon";
    case "bmp":
      return "image/bmp";
    case "avif":
      return "image/avif";
    default:
      return "application/octet-stream";
  }
}
