/**
 * git 变更文件状态徽标配色:提交详情面板 / 提交对话框的文件列表共用;
 * U = 未跟踪(仅工作区变更预览会出现)
 */
export function statusClass(status: string): string {
  switch (status) {
    case "A":
      return "text-green-600 dark:text-green-400";
    case "D":
      return "text-red-600 dark:text-red-400";
    case "R":
      return "text-blue-600 dark:text-blue-400";
    case "T":
      return "text-purple-600 dark:text-purple-400";
    case "U":
      return "text-sky-600 dark:text-sky-400";
    default:
      return "text-amber-600 dark:text-amber-400";
  }
}
