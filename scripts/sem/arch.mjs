// sidecar 架构识别:只看文件头(Mach-O / PE / ELF),不依赖 `file` / `lipo`。
// 这两个命令只在部分系统上存在,而 `file` 的退出码恒为 0,不能当门禁用。
//
// 纯函数模块,便于单测:synthetic header 测过即可覆盖 arm64/x86_64 判定与失败路径。

const MACHO_CPU_TYPES = {
  0x0100000c: "arm64", // CPU_TYPE_ARM64
  0x01000007: "x86_64", // CPU_TYPE_X86_64
  0x0000000c: "arm",
  0x00000007: "x86",
};

/// 三元组 → 期望架构(只覆盖 manifest.json 里存在的目标)
export function expectedArch(triple) {
  if (triple.startsWith("aarch64-")) return "arm64";
  if (triple.startsWith("x86_64-")) return "x86_64";
  if (triple.startsWith("i686-")) return "x86";
  if (triple.startsWith("armv7-")) return "arm";
  return null;
}

/// Mach-O(含 FAT/universal):返回首个切片架构
function machoArch(buffer) {
  const magic = buffer.readUInt32BE(0);
  // 0xCAFEBABE = FAT;0xBEBAFECA = FAT_CIGAM
  if (magic === 0xcafebabe || magic === 0xbebafeca) {
    const count = buffer.readUInt32BE(4);
    if (count === 0) return "unknown";
    // fat_arch 表从 8 开始,每项 20 字节:cputype cpusubtype offset size align
    const cputype = buffer.readUInt32BE(8);
    return MACHO_CPU_TYPES[cputype] ?? `unknown cputype 0x${cputype.toString(16)}`;
  }
  if (magic === 0xfeedfacf || magic === 0xcffaedfe) {
    // 64 位头:cputype 在偏移 4,Go/sem 的 arm64 二进制读到 cffaedfe → 按 LE 解析
    const cputype = buffer.readUInt32LE(4);
    return MACHO_CPU_TYPES[cputype] ?? `unknown cputype 0x${cputype.toString(16)}`;
  }
  if (magic === 0xfeedface || magic === 0xcefaedfe) {
    const cputype = buffer.readUInt32LE(4);
    return MACHO_CPU_TYPES[cputype] ?? `unknown cputype 0x${cputype.toString(16)}`;
  }
  return null;
}

/// ELF:e_machine 在 18 字节处(小端)
function elfArch(buffer) {
  const machine = buffer.readUInt16LE(18);
  if (machine === 0xb7) return "arm64";
  if (machine === 0x3e) return "x86_64";
  if (machine === 0x28) return "arm";
  if (machine === 0x03) return "x86";
  return `unknown e_machine 0x${machine.toString(16)}`;
}

/// PE/MZ:COFF Machine 字段(PE 头偏移在 0x3c)
function peArch(buffer) {
  const peOffset = buffer.readUInt32LE(0x3c);
  if (buffer.length < peOffset + 6) return "unknown (truncated PE header)";
  const signature = buffer.toString("ascii", peOffset, peOffset + 4);
  if (signature !== "PE\0\0") return `unknown (bad PE signature ${JSON.stringify(signature)})`;
  const machine = buffer.readUInt16LE(peOffset + 4);
  if (machine === 0x8664) return "x86_64";
  if (machine === 0xaa64) return "arm64";
  if (machine === 0x014c) return "x86";
  if (machine === 0x01c4) return "arm";
  return `unknown machine 0x${machine.toString(16)}`;
}

/// 返回识别到的架构字符串,或 `null`(完全无法识别的格式)
export function detectBinaryArch(buffer) {
  if (buffer.length >= 4 && buffer.toString("hex", 0, 4).startsWith("7f454c46")) {
    return elfArch(buffer);
  }
  if (buffer.length >= 2 && buffer.toString("ascii", 0, 2) === "MZ") return peArch(buffer);
  return machoArch(buffer);
}

/// 校验 `target` 三元组的期望架构与实际文件头是否一致;不一致时抛出带上下文的错误。
/// 返回实际架构字符串。
export function assertArchMatches(buffer, target) {
  const expected = expectedArch(target);
  if (!expected) throw new Error(`无法从三元组推断架构: ${target}`);
  if (buffer.length < 32) throw new Error("文件过小,不是有效可执行文件");
  const actual = detectBinaryArch(buffer);
  if (actual !== expected) {
    const header = buffer.length >= 4 ? buffer.toString("hex", 0, 4) : "unknown";
    throw new Error(
      `sem sidecar 架构不匹配: target=${target} 期望 ${expected},实际 ${actual ?? "未知格式"}` +
        ` (文件头 ${header})`,
    );
  }
  return actual;
}
