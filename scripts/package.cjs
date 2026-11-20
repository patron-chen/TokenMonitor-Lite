'use strict';

const { spawnSync } = require('node:child_process');
const { copyFileSync, mkdirSync, mkdtempSync, readFileSync } = require('node:fs');
const { isAbsolute, join, resolve } = require('node:path');
const { ensureTokscale } = require('./tokscale.cjs');
const { version } = require('../package.json');

const root = resolve(__dirname, '..');

function run(program, args)
{
  const result = spawnSync(program, args, { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(program + ' exited with code ' + result.status);
}

async function packageWindows()
{
  const scanner = await ensureTokscale();
  run(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), 'build']);
  run('cargo.exe', ['build', '--manifest-path', 'src-tauri/Cargo.toml', '--release', '--locked', '--features', 'custom-protocol']);

  const custom = process.env.CARGO_TARGET_DIR;
  const target = custom ? (isAbsolute(custom) ? custom : resolve(root, custom)) : join(root, 'src-tauri/target');
  const binary = join(target, 'release/token-monitor-lite.exe');
  const exe = readFileSync(binary);
  const pe = exe.readUInt32LE(0x3c);
  if (exe.readUInt16LE(pe + 4) !== 0x8664)
  {
    throw new Error('The compiled binary is not Windows x64.');
  }

  const name = 'Token-Monitor-Lite-' + version + '-win-x64';
  const stages = join(root, 'tmp/packages');
  const release = join(root, 'release');
  mkdirSync(stages, { recursive: true });
  mkdirSync(release, { recursive: true });
  // A running portable copy can lock its executable on Windows.
  const out = mkdtempSync(join(stages, 'win-x64-'));

  const files = new Map([
    ['TokenMonitorLite.exe', binary],
    ['tokscale.exe', scanner],
    ['README.md', join(root, 'README.md')],
    ['LICENSE-Token-Monitor.txt', join(root, 'LICENSE')],
    ['LICENSE-Tokscale.txt', join(root, 'licenses/Tokscale.txt')],
    ['LICENSE-LiteLLM.txt', join(root, 'licenses/LiteLLM.txt')],
    ['ICON-NOTICES.md', join(root, 'licenses/ICON-NOTICES.md')]
  ]);
  for (const [name, file] of files)
  {
    copyFileSync(file, join(out, name));
  }

  const archive = join(release, name + '.zip');
  run('tar.exe', ['-a', '-c', '-f', archive, '-C', out, ...files.keys()]);
  console.log(archive);
}

packageWindows().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});
