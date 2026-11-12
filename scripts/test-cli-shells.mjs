/** CLI shell tests with GitHub Actions' PowerShell exit-code wrapper. */
import assert from 'node:assert/strict';
import { readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { getExe, run, strictUTF8, withTemp } from './cli-test-util.mjs';

const exe = getExe();
const script = fileURLToPath(new URL('./test-cli.ps1', import.meta.url));
const quote = path => `'${path.replaceAll("'", "''")}'`;
const results = [];
for (const shell of ['powershell.exe', 'pwsh.exe']) {
  for (const scenario of ['success', 'assertion_failure', 'cleanup_failure']) {
    const result = await withTemp('repomeow-actions-', async root => {
      const wrapper = join(root, 'step.ps1');
      // Match the runner without resetting LASTEXITCODE in the wrapper.
      writeFileSync(wrapper, '\uFEFF' + "$ErrorActionPreference = 'stop'\n"
        + (scenario === 'cleanup_failure' ? "function Remove-Item { Write-Error 'Injected cleanup failure' }\n" : '')
        + `& ${quote(script)} -Exe ${quote(exe)}`
        + (scenario === 'assertion_failure' ? ' -InjectAssertionFailure' : '')
        + '\nif ((Test-Path -LiteralPath variable:\\LASTEXITCODE)) { exit $LASTEXITCODE }\n');
      const p = await run(shell, ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        // Host diagnostics also need UTF-8 on localized Windows PowerShell 5.1.
        '-Command', `[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); . ${quote(wrapper)}`],
        { env: { ...process.env, TEMP: root, TMP: root } });
      const stdout = strictUTF8(p.stdout);
      const stderr = strictUTF8(p.stderr);
      if (scenario !== 'success') {
        assert.equal(p.code, 1, 'Failure exit code changed');
        assert(stderr.includes('Injected ' + scenario.replaceAll('_', ' ')), stderr);
        assert.equal(stdout.trim(), '', 'Failure reported success');
      } else {
        assert.equal(p.code, 0, stderr);
        assert.equal(stderr, '');
        assert.equal(JSON.parse(stdout).passed, true);
      }
      const remaining = readdirSync(root).filter(name => name.startsWith('repomeow-shell-'));
      if (scenario === 'cleanup_failure') {
        assert(remaining.length > 0, 'Cleanup failure was not exercised');
      } else {
        assert.equal(remaining.length, 0, 'Test data was not cleaned up');
      }
      return { shell, scenario, exit: p.code };
    });
    results.push(result);
  }
}
console.log(JSON.stringify({ passed: true, cases: results }));
