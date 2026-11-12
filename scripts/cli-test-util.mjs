import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { parseArgs } from 'node:util';

export function getExe() {
  const { values } = parseArgs({ options: { exe: { type: 'string' } } });
  assert(values.exe, 'Usage: --exe <built or installed repomeow-cli executable>');
  return realpathSync(values.exe);
}

// Buffer.toString silently replaces invalid bytes; the CLI contract must reject them.
export function strictUTF8(bytes) {
  return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
}

export function run(file, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = execFile(file, args.map(String), {
      ...options, encoding: 'buffer', timeout: 60_000,
      maxBuffer: 16 * 1024 * 1024, windowsHide: true, shell: false,
    }, (error, stdout, stderr) => {
      // Nonzero CLI exits are test results; launch errors, signals and timeouts fail.
      if (error && (typeof error.code !== 'number' || error.killed || error.signal)) {
        reject(error);
      } else {
        resolve({ code: error?.code ?? 0, stdout, stderr });
      }
    });
    child.stdin.end();
  });
}

export async function withTemp(prefix, action) {
  const parent = realpathSync(tmpdir());
  const root = mkdtempSync(join(parent, prefix));
  try {
    return await action(root);
  } finally {
    // Delete only the resolved, directly owned temporary directory.
    assert.equal(dirname(realpathSync(root)), parent, 'Unsafe cleanup path');
    rmSync(root, { recursive: true, force: true });
  }
}

export async function parallel(actions) {
  // Collect every child before propagating errors or deleting its data directory.
  const results = await Promise.allSettled(actions);
  const failed = results.find(result => result.status === 'rejected');
  if (failed) throw failed.reason;
  return results.map(result => result.value);
}
