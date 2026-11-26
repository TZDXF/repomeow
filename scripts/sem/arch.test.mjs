// 架构识别回归:用合成文件头覆盖 macOS arm64/x86_64、universal、ELF、PE 与两类失败路径。
// 跑法:`pnpm sem:verify:test`(node --test)。CI 的 macos job 与本地复跑都会执行。
import assert from "node:assert/strict";
import { test } from "node:test";

import { MIN_BINARY_SIZE, assertArchMatches, detectBinaryArch, expectedArch } from "./arch.mjs";

/// 64 位 Mach-O 头:magic(4) cputype(4) cpusubtype(4) filetype(4) … 补到 32 字节
function macho64(cputype, bigEndianMagic = false) {
  const buffer = Buffer.alloc(64);
  buffer.writeUInt32LE(0xfeedfacf, 0); // MH_MAGIC_64,小端写入
  if (bigEndianMagic) buffer.writeUInt32BE(0xcffaedfe, 0);
  buffer.writeUInt32LE(cputype, 4);
  buffer.writeUInt32LE(0, 8);
  buffer.writeUInt32LE(2, 12); // MH_EXECUTE
  return buffer;
}

/// universal(FAT)头:magic + nfat_arch + 一项 fat_arch
function fatBinary(cputype) {
  const buffer = Buffer.alloc(64);
  buffer.writeUInt32BE(0xcafebabe, 0);
  buffer.writeUInt32BE(1, 4);
  buffer.writeUInt32BE(cputype, 8);
  buffer.writeUInt32BE(0, 12);
  buffer.writeUInt32BE(4096, 16);
  buffer.writeUInt32BE(1024, 20);
  buffer.writeUInt32BE(12, 24);
  return buffer;
}

function elf(machine) {
  const buffer = Buffer.alloc(64);
  buffer.write("7f454c46", 0, "hex"); // \x7fELF
  buffer.writeUInt8(2, 4); // ELFCLASS64
  buffer.writeUInt8(1, 5); // little endian
  buffer.writeUInt16LE(machine, 18);
  return buffer;
}

function pe(machine) {
  const buffer = Buffer.alloc(256);
  buffer.write("MZ", 0, "ascii");
  buffer.writeUInt32LE(0x80, 0x3c);
  buffer.write("PE\0\0", 0x80, "ascii");
  buffer.writeUInt16LE(machine, 0x84);
  return buffer;
}

/// 40 字节的 `MZ`:刚够写 magic + e_lfanew 之前就断了(e_lfanew 在 0x3c..0x40)
function truncatedPe() {
  const buffer = Buffer.alloc(40);
  buffer.write("MZ", 0, "ascii");
  return buffer;
}

/// 16 字节的 ELF:magic 与 class/data 都在,但 e_machine(18..20) 已经落在文件外
function truncatedElf() {
  const buffer = Buffer.alloc(16);
  buffer.write("7f454c46", 0, "hex");
  return buffer;
}

test("三元组映射到期望架构", () => {
  assert.equal(expectedArch("aarch64-apple-darwin"), "arm64");
  assert.equal(expectedArch("x86_64-apple-darwin"), "x86_64");
  assert.equal(expectedArch("x86_64-pc-windows-msvc"), "x86_64");
  assert.equal(expectedArch("aarch64-unknown-linux-gnu"), "arm64");
  assert.equal(expectedArch("mips-unknown-linux-gnu"), null);
});

test("识别 Mach-O arm64 与 x86_64", () => {
  assert.equal(detectBinaryArch(macho64(0x0100000c)), "arm64");
  assert.equal(detectBinaryArch(macho64(0x01000007)), "x86_64");
  // Go 的 arm64 二进制文件头读出来是 cffaedfe,同样按 LE 解析
  assert.equal(detectBinaryArch(macho64(0x0100000c, true)), "arm64");
});

test("识别 universal 二进制的首个切片", () => {
  assert.equal(detectBinaryArch(fatBinary(0x0100000c)), "arm64");
  assert.equal(detectBinaryArch(fatBinary(0x01000007)), "x86_64");
});

test("识别 ELF 与 PE", () => {
  assert.equal(detectBinaryArch(elf(0x3e)), "x86_64");
  assert.equal(detectBinaryArch(elf(0xb7)), "arm64");
  assert.equal(detectBinaryArch(pe(0x8664)), "x86_64");
  assert.equal(detectBinaryArch(pe(0xaa64)), "arm64");
});

test("架构不一致必须失败,不能只打印警告", () => {
  // arm64 二进制放在 x86_64 目标名下 → 抛错(这是修复前的门禁漏洞)
  assert.throws(
    () => assertArchMatches(macho64(0x0100000c), "x86_64-apple-darwin"),
    /架构不匹配: target=x86_64-apple-darwin 期望 x86_64,实际 arm64/,
  );
  assert.throws(
    () => assertArchMatches(macho64(0x01000007), "aarch64-apple-darwin"),
    /架构不匹配: target=aarch64-apple-darwin 期望 arm64,实际 x86_64/,
  );
  assert.throws(() => assertArchMatches(elf(0x3e), "aarch64-unknown-linux-gnu"), /架构不匹配/);
});

test("架构一致时返回实际架构", () => {
  assert.equal(assertArchMatches(macho64(0x0100000c), "aarch64-apple-darwin"), "arm64");
  assert.equal(assertArchMatches(pe(0x8664), "x86_64-pc-windows-msvc"), "x86_64");
});

test("无法推断的三元组与过小文件都失败", () => {
  assert.throws(() => assertArchMatches(macho64(0x0100000c), "riscv64gc-unknown-linux-gnu"), /无法从三元组推断架构/);
  assert.throws(() => assertArchMatches(Buffer.alloc(8), "aarch64-apple-darwin"), /文件过小/);
});

// 长度门限必须高于「读 PE 偏移所需的 0x40 字节」,否则 40 字节的 `MZ` 会在 readUInt32LE(0x3c)
// 处抛 `RangeError: … Received 60`,报错语义从「文件被截断」退化成 Buffer 内部错误,
// 而这正是残文件/中断下载最真实的形态。反向体测:把 MIN_BINARY_SIZE 调回 32 即红。
test("截断的 PE/ELF 头报「文件过小」,不抛 RangeError", () => {
  assert.equal(MIN_BINARY_SIZE, 0x40);
  assert.throws(
    () => assertArchMatches(truncatedPe(), "x86_64-pc-windows-msvc"),
    (error) =>
      error instanceof Error &&
      !(error instanceof RangeError) &&
      /文件过小/.test(error.message) &&
      /40/.test(error.message),
  );
  // 直接调识别函数(不经过 assertArchMatches 的长度门限)也不能抛 RangeError,
  // 否则「先判长度再读」的修宪只在 assertArchMatches 这一个入口成立。
  assert.equal(detectBinaryArch(truncatedPe()), "unknown (truncated PE header)");
  assert.equal(detectBinaryArch(truncatedElf()), "unknown (truncated ELF header)");
  assert.equal(detectBinaryArch(Buffer.alloc(5)), "unknown (truncated Mach-O header)");
  // 刚好达到门限但 e_lfanew 指到文件外 → 仍走签名校验的路径,不抛 RangeError
  const edge = Buffer.alloc(MIN_BINARY_SIZE);
  edge.write("MZ", 0, "ascii");
  edge.writeUInt32LE(0x100, 0x3c);
  assert.equal(detectBinaryArch(edge), "unknown (truncated PE header)");
});
