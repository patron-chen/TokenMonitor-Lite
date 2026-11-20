'use strict';

const { createHash } = require('node:crypto');
const { execFile } = require('node:child_process');
const { existsSync, mkdirSync, readFileSync, renameSync, unlinkSync } = require('node:fs');
const { join } = require('node:path');
const { promisify } = require('node:util');
const pin = require('./tokscale.json');

const directory = join(__dirname, '..', 'vendor');
const scanner = join(directory, 'tokscale.exe');
const maxBytes = 50 * 1024 * 1024;
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const run = promisify(execFile);

async function download(url, staged)
{
  const powershell = join(process.env.SystemRoot, 'System32/WindowsPowerShell/v1.0/powershell.exe');
  const helper = readFileSync(join(__dirname, 'windows-http.ps1'), 'utf8');
  const command = helper + "\n$ErrorActionPreference = 'Stop'\n" +
    'Invoke-LiteHttpRequest -Uri $env:LITE_DOWNLOAD_URL -OutFile $env:LITE_DOWNLOAD_PATH ' +
    '-MaxBytes $env:LITE_DOWNLOAD_LIMIT -TimeoutMs $env:LITE_DOWNLOAD_TIMEOUT | ConvertTo-Json -Compress';
  const deadline = Date.now() + 60_000;
  // A GitHub redirect can change hosts. Resolve proxy and bypass rules again at each hop.
  for (let redirects = 0; redirects <= 10; redirects++)
  {
    const timeout = deadline - Date.now();
    if (timeout <= 0) throw new Error('Tokscale download timed out.');
    if (url.protocol !== 'https:') throw new Error('Tokscale download requires HTTPS.');
    let reply;
    try
    {
      const { stdout } = await run(powershell, ['-NoProfile', '-NonInteractive', '-Command', command], {
        windowsHide: true, timeout,
        env: { ...process.env, LITE_DOWNLOAD_URL: url.href, LITE_DOWNLOAD_PATH: staged,
          LITE_DOWNLOAD_LIMIT: String(maxBytes), LITE_DOWNLOAD_TIMEOUT: String(timeout) }
      });
      reply = JSON.parse(stdout);
    }
    catch
    {
      throw new Error(`Tokscale download failed while connecting to ${url.hostname}.`);
    }
    if (reply.Status === 200) return;
    if (![301, 302, 303, 307, 308].includes(reply.Status) || !reply.Location)
    {
      throw new Error(`Tokscale download failed (HTTP ${reply.Status}): ${url.hostname}`);
    }
    url = new URL(reply.Location, url);
  }
  throw new Error('Too many Tokscale download redirects.');
}

async function ensureTokscale()
{
  if (process.platform !== 'win32' || process.arch !== 'x64')
  {
    throw new Error('The pinned Tokscale scanner requires Windows x64.');
  }
  if (existsSync(scanner) && sha256(readFileSync(scanner)) === pin.sha256)
  {
    console.log(`Using Tokscale ${pin.version} (${pin.releaseTag})`);
    return scanner;
  }

  const url = `https://github.com/${pin.releaseRepo}/releases/download/${pin.releaseTag}/${pin.asset}`;
  // Replace only after the verified download is fully written.
  mkdirSync(directory, { recursive: true });
  const staged = join(directory, `tokscale.${process.pid}.tmp`);
  try
  {
    await download(new URL(url), staged);
    const bytes = readFileSync(staged);
    if (bytes.length > maxBytes || sha256(bytes) !== pin.sha256)
    {
      throw new Error(`Tokscale SHA-256 mismatch: ${url}`);
    }
    renameSync(staged, scanner);
  }
  finally
  {
    if (existsSync(staged)) unlinkSync(staged);
  }
  console.log(`Installed Tokscale ${pin.version} (${pin.releaseTag})`);
  return scanner;
}

if (require.main === module)
{
  ensureTokscale().catch(error => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

module.exports = { ensureTokscale };
