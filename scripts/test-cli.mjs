/** Release console regression: --exe <built or installed repomeow-cli>. */
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { delimiter, dirname, join } from 'node:path';
import { getExe, parallel, run, strictUTF8, withTemp } from './cli-test-util.mjs';

const exe = getExe();
if (process.platform === 'win32') {
  const binary = readFileSync(exe);
  const pe = binary.readUInt32LE(0x3c);
  assert.equal(binary.readUInt16LE(pe + 24 + 68), 3, 'Expected console subsystem');
}
assert.throws(() => strictUTF8(Buffer.from([0xc3, 0x28])), TypeError);

const result = await withTemp('repomeow-console-', async root => {
  const data = join(root, 'data');
  const env = { ...process.env, REPOMEOW_DATA_DIR: data };
  let calls = 0;
  async function call(command, code = 0, selectedEnv = env) {
    const p = await run(exe, command, { env: selectedEnv });
    assert.equal(p.code, code, JSON.stringify({ command, ...p }));
    const stdout = strictUTF8(p.stdout);
    const stderr = strictUTF8(p.stderr);
    calls += 1;
    if (code) {
      assert.equal(p.stdout.length, 0, stdout);
      const failure = JSON.parse(stderr);
      assert.deepEqual(Object.keys(failure).sort(), ['code', 'detail', 'message']);
      return failure;
    }
    assert.equal(p.stderr.length, 0, stderr);
    return ['--help', '--version'].includes(command[0]) ? stdout : JSON.parse(stdout);
  }

  await call(['--version']);
  await call(['--help']);
  assert(!existsSync(data), 'Help/version should not initialize business data');
  assert.equal((await call(['project', 'add'], 2)).code, 'invalid_arguments');
  assert(!existsSync(data), 'Parse failures should not publish invalidation');
  assert.equal((await call(['unknown-command'], 2)).code, 'invalid_arguments');
  assert.equal((await call(['project', 'get', '-p', join(root, 'missing')], 1)).code, 'project_not_found');
  const project = join(root, '中文项目');
  mkdirSync(project);
  const added = await call(['project', 'add', '-p', project, '--name', '中文项目', '--description', '初始']);
  assert(JSON.stringify(added).includes('中文项目'));
  await call(['project', 'update', '-p', project, '--name', '更新名称', '--description', '更新']);
  assert(JSON.stringify(await call(['project', 'list'])).includes('更新名称'));
  const tag = await call(['tag', 'create', '--name', '中文标签']);
  await call(['tag', 'update', '--id', tag.id, '--name', '中文标签', '--color', '#ff0000']);
  await call(['tag', 'set-project', '-p', project, '--tag-ids', tag.id]);
  const script = await call(['script', 'create', '-p', project, '--name', 'custom', '--command', 'echo first']);
  await call(['script', 'update', '--id', script.id, '--name', 'custom', '--command', 'echo updated']);
  await call(['pin', 'set', '-p', project, '--kind', 'customCommand', '--target-key', script.id,
    '--pinned', 'true', '--label', 'custom', '--command', 'echo updated']);
  await call(['hidden', 'set', '-p', project, '--kind', 'packageFile', '--target-key', 'package.json', '--hidden', 'true']);
  await call(['hidden', 'set', '-p', project, '--kind', 'packageFile', '--target-key', 'package.json', '--hidden', 'false']);
  await call(['script', 'delete', '--id', script.id]);
  const beforeFailure = statSync(join(data, 'projects.db')).size;
  await call(['tag', 'create', '--name', '中文标签'], 1);
  assert(statSync(join(data, 'projects.db')).size >= beforeFailure);
  await call(['project', 'archive', '-p', project]);
  assert.equal((await call(['project', 'list'])).projects.length, 0);
  await call(['project', 'unarchive', '-p', project]);
  await call(['file', 'save', '-p', project, '--path', 'test.txt', '--content', '中文 UTF-8']);
  assert.deepEqual(readFileSync(join(project, 'test.txt')), Buffer.from('中文 UTF-8'));
  await call(['file', 'save', '-p', project, '--path', '../escape.txt', '--content', 'bad'], 1);
  assert(!existsSync(join(root, 'escape.txt')));
  const config = await call(['ai', 'config-get']);
  const validConfig = join(data, 'ai-config.json');
  const initial = readFileSync(validConfig);
  const invalid = join(root, 'invalid.json');
  writeFileSync(invalid, '{');
  await call(['ai', 'config-save', '--file', invalid], 1);
  assert.deepEqual(readFileSync(validConfig), initial);
  const valid = join(root, 'config.json');
  writeFileSync(valid, JSON.stringify(config));
  await call(['ai', 'config-save', '--file', valid]);
  const schedules = join(root, 'schedules.json');
  writeFileSync(schedules, '[]');
  await call(['report', 'schedules-save', '--file', schedules]);
  await call(['report', 'system-schedule-save', '--enabled', 'false', '--interval-minutes', '3']);
  await call(['ai', 'usage-clear']);
  const server = createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end('{"username":"local-fixture"}');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  try {
    const account = await call(['account', 'add', '--provider', 'gitlab', '--label', 'fixture',
      '--base-url', `http://127.0.0.1:${server.address().port}`, '--token', 'test-only-token']);
    await call(['account', 'update', '--id', account.id, '--label', 'updated']);
    await call(['account', 'remove', '--id', account.id]);
    assert.equal((await call(['account', 'list'])).accounts.length, 0);
  } finally {
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
  await call(['wiki', 'config-set', '-p', project, '--concurrency', '3']);
  await call(['wiki', 'delete', '-p', project]);

  // Independent console processes contend on the same data root.
  await parallel(Array.from({ length: 8 }, (_, n) => call(['tag', 'create', '--name', `parallel-${n}`])));
  assert.equal((await call(['tag', 'list'])).tags.length, 9);
  const commit = join(root, 'commit.md');
  const report = join(root, 'report.md');
  writeFileSync(commit, '提交提示词');
  writeFileSync(report, '报告提示词');
  await parallel([
    call(['prompt', 'set', '--commit-file', commit]),
    call(['prompt', 'set', '--report-file', report]),
  ]);
  const prompts = await call(['prompt', 'get']);
  assert.equal(prompts.commit, '提交提示词');
  assert.equal(prompts.report, '报告提示词');
  await parallel(Array.from({ length: 4 }, (_, n) => call(['file', 'save', '-p', project,
    '--path', 'test.txt', '--content', String(n).repeat(10_000)])));
  assert(Array.from({ length: 4 }, (_, n) => String(n).repeat(10_000))
    .includes(strictUTF8(readFileSync(join(project, 'test.txt')))));
  const otherEnv = { ...env, REPOMEOW_DATA_DIR: join(root, 'other') };
  assert.equal((await call(['project', 'list'], 0, otherEnv)).projects.length, 0);
  assert((await call(['project', 'list'])).projects.length > 0);
  if (process.platform === 'win32') {
    // Windows environment names are case insensitive; avoid duplicate Path/PATH.
    const pathKey = Object.keys(env).find(key => key.toLowerCase() === 'path') ?? 'PATH';
    const shellEnv = { ...env, [pathKey]: dirname(exe) + delimiter + (env[pathKey] ?? '') };
    for (const [command, code] of [['project list', 0], ['project add', 2], ['project get -p missing-project', 1]]) {
      const p = await run(process.env.ComSpec ?? 'cmd.exe',
        ['/d', '/s', '/c', `repomeow-cli ${command} >out.json 2>err.json`], { env: shellEnv, cwd: root });
      assert.equal(p.code, code);
      const out = strictUTF8(readFileSync(join(root, 'out.json')));
      const err = strictUTF8(readFileSync(join(root, 'err.json')));
      if (code) {
        assert.equal(out, '');
        assert(JSON.parse(err).code);
      } else {
        assert.equal(err, '');
        assert.equal(JSON.parse(out).projects[0].name, '更新名称');
      }
      calls += 1;
    }
  }
  const revisions = JSON.parse(strictUTF8(readFileSync(join(data, 'cli-revisions.json'))));
  assert(['project', 'tag', 'file', 'wiki', 'ai', 'prompt'].every(domain => domain in revisions));
  await call(['tag', 'delete', '--id', tag.id]);
  await call(['project', 'delete', '-p', project]);
  assert.equal((await call(['project', 'list'])).projects.length, 0);
  assert.equal(calls, process.platform === 'win32' ? 59 : 56);
  return { passed: true, calls, exe, tag };
});
console.log(JSON.stringify(result));
