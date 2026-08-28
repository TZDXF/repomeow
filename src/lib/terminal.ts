import { cmd } from "@/lib/tauri";
import type { TerminalKind } from "@/stores/settings";

export interface TerminalCapabilities {
  isWindows: boolean;
  windowsTerminal: boolean;
  shells: Record<TerminalKind, boolean>;
}

/** 探测 Windows 终端宿主与各 Shell 的可用性；每次进入设置页重新读取当前环境。 */
export function getTerminalCapabilities(): Promise<TerminalCapabilities | null> {
  return cmd<TerminalCapabilities>("detect_terminal_capabilities").catch(() => null);
}
