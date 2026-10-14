const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { test } = require('node:test');

const script = resolve(__dirname, 'check-enhancement.sh');

function run(t, args, failCommand) {
  const dir = mkdtempSync(join(tmpdir(), 'meetily-check-'));
  const log = join(dir, 'commands');
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const command of ['node', 'bun', 'pnpm', 'cargo']) {
    writeFileSync(join(dir, command), `#!/bin/sh\nprintf '%s\\n' '${command}' >> "$COMMAND_LOG"\npwd >> "$DIRECTORY_LOG"\nexit ${command === failCommand ? 73 : 0}\n`, { mode: 0o755 });
  }
  const result = spawnSync('/bin/bash', [script, ...args], {
    cwd: tmpdir(), encoding: 'utf8',
    env: { ...process.env, PATH: `${dir}:/usr/bin:/bin`, COMMAND_LOG: log, DIRECTORY_LOG: join(dir, 'directories') },
  });
  return { result, commands: require('node:fs').existsSync(log) ? require('node:fs').readFileSync(log, 'utf8').trim().split('\n') : [], directories: require('node:fs').existsSync(join(dir, 'directories')) ? require('node:fs').readFileSync(join(dir, 'directories'), 'utf8').trim().split('\n') : [] };
}

test('rejects unknown stages before invoking build tools', (t) => {
  const { result, commands } = run(t, ['--typo']);
  assert.equal(result.status, 2, result.stderr);
  assert.deepEqual(commands, []);
});

test('frontend failure stops subsequent checks and returns the failing exit code', (t) => {
  const { result, commands } = run(t, ['--frontend'], 'bun');
  assert.equal(result.status, 73, result.stderr);
  assert.deepEqual(commands, ['pnpm', 'node', 'bun']);
});

test('frontend checks resolve the repository independently of the caller directory', (t) => {
  const { result, commands, directories } = run(t, ['--frontend']);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(commands, ['pnpm', 'node', 'bun', 'pnpm', 'pnpm', 'pnpm', 'pnpm', 'pnpm']);
  assert.deepEqual(directories, Array(8).fill(resolve(__dirname, '..')));
});
