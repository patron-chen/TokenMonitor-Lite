'use strict';

const { createHash } = require('node:crypto');
const { existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } = require('node:fs');
const { join } = require('node:path');
const pin = require('./tokscale.json');

const directory = join(__dirname, '..', 'vendor');
const scanner = join(directory, 'tokscale.exe');
const maxBytes = 50 * 1024 * 1024;
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

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
  const response = await fetch(url, { signal: AbortSignal.timeout(60_000) });
  if (!response.ok) throw new Error(`Tokscale download failed (${response.status}): ${url}`);
  if (Number(response.headers.get('content-length')) > maxBytes) throw new Error('Tokscale download exceeds 50 MiB.');
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length > maxBytes || sha256(bytes) !== pin.sha256)
  {
    throw new Error(`Tokscale SHA-256 mismatch: ${url}`);
  }

  // Replace only after the verified download is fully written.
  mkdirSync(directory, { recursive: true });
  const staged = join(directory, `tokscale.${process.pid}.tmp`);
  try
  {
    writeFileSync(staged, bytes);
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
