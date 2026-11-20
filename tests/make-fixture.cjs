'use strict';

const { appendFileSync, existsSync, mkdirSync, writeFileSync } = require('node:fs');
const { join, resolve } = require('node:path');
const { randomUUID } = require('node:crypto');
const { DatabaseSync } = require('node:sqlite');
const { zstdCompressSync } = require('node:zlib');

const root = resolve(process.argv[2] ?? 'tmp/token-monitor-lite/fixture');
if (existsSync(root))
{
  throw new Error('Choose a new fixture directory.');
}
const now = new Date();
const older = new Date(now.getFullYear(), now.getMonth() - 1, 15, 12);
const routedRequestId = 'fixture-routed-response';

function writeCodex(at, label)
{
  const id = randomUUID();
  const [year, month, day] = at.toISOString().slice(0, 10).split('-');
  const dir = join(root, '.codex/sessions', year, month, day);
  const file = join(dir, 'rollout-' + at.toISOString().slice(0, 19).replace(/:/g, '-') + '-' + id + '.jsonl');
  const longReply = label === 'warm-write' || label === '1';
  const output = longReply ? 210 : 10;
  const usage = { input_tokens: 100, output_tokens: output, cached_input_tokens: 20, reasoning_output_tokens: longReply ? 40 : 4 };
  const responseId = label === '1' ? routedRequestId : 'fixture-' + id;
  const rows = [
    [-1000, 'session_meta', { id, cwd: 'C:/fixture' }],
    [-1000, 'event_msg', { type: 'task_started' }],
    [-1000, 'turn_context', { model: 'gpt-6.2-sol', effort: 'high' }],
    [-1000, 'event_msg', { type: 'user_message', message: 'Inspect fixture ' + label }],
    [-50, 'event_msg', { type: 'item_completed', item: { type: 'AgentMessage' }, started_at_ms: at.getTime() - 800 }],
    [0, 'token_usage_record', { response_id: responseId, usage }],
    [0, 'event_msg', { type: 'token_count', info: {
      model_context_window: 400, last_token_usage: { ...usage, total_tokens: 100 + output }, total_token_usage: usage
    }, rate_limits: { limit_id: 'codex', plan_type: 'prolite', primary: {
      used_percent: 25, window_minutes: 10080, resets_at: Math.floor(now.getTime() / 1000) + 86400
    } } }],
    [0, 'event_msg', { type: 'task_complete' }]
  ];
  mkdirSync(dir, { recursive: true });
  writeFileSync(file, rows.map(([offset, type, payload]) => JSON.stringify({
    timestamp: new Date(at.getTime() + offset).toISOString(), type, payload
  })).join('\n') + '\n');
  const index = join(root, '.codex/session_index.jsonl');
  appendFileSync(index, JSON.stringify({ id, thread_name: 'Fixture ' + label, updated_at: at.toISOString() }) + '\n');
  return file;
}

function int(value)
{
  let remaining = BigInt(value);
  const bytes = [];
  while (remaining > 127n)
  {
    bytes.push(Number(remaining & 127n) | 128);
    remaining >>= 7n;
  }
  bytes.push(Number(remaining));
  return Buffer.from(bytes);
}

function field(tag, value)
{
  if (typeof value === 'number') return Buffer.concat([int(tag * 8), int(value)]);
  const bytes = Buffer.from(value);
  return Buffer.concat([int(tag * 8 + 2), int(bytes.length), bytes]);
}

function writeAntigravity(source)
{
  const dir = join(root, '.gemini', source, 'conversations');
  mkdirSync(dir, { recursive: true });
  const db = new DatabaseSync(join(dir, randomUUID() + '.db'));
  db.exec('PRAGMA journal_mode = WAL; CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB); '
    + 'CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER, metadata BLOB, step_payload BLOB);');
  const response = source + '-response';
  const usage = Buffer.concat([field(1, 100), field(2, 20), field(3, 9), field(5, 12), field(9, 7), field(10, 2), field(11, response)]);
  const timestamp = field(1, Math.floor(now.getTime() / 1000));
  const generator = field(1, Buffer.concat([field(4, usage), field(9, field(4, timestamp)),
    field(11, field(2, 250000000)), field(12, field(2, 750000000)), field(19, 'gemini-3-flash-a')]));
  const user = field(19, field(1, '<USER_REQUEST>Inspect the Antigravity fixture</USER_REQUEST>'));
  const metadata = Buffer.concat([field(1, timestamp), field(9, field(11, response))]);
  const planner = field(20, field(7, field(2, 'list_directory')));
  db.prepare('INSERT INTO steps VALUES (0, 14, ?, ?)').run(field(3, 4), user);
  db.prepare('INSERT INTO steps VALUES (1, 15, ?, ?)').run(metadata, planner);
  db.prepare('INSERT INTO gen_metadata VALUES (0, ?)').run(generator);
  db.close();
}

function writeDsh()
{
  const id = randomUUID();
  const dir = join(root, '.dsh/sessions/workspace', id);
  mkdirSync(dir, { recursive: true });
  const rows = [
    { type: 'session', id, version: 3, createdAt: now.getTime(), cwd: 'C:/fixture' },
    {
      type: 'user/message', seq: 1, time: now.getTime(),
      data: { source: { kind: 'user' }, content: [{ type: 'text', text: 'Inspect the DeepSeek fixture' }] }
    },
    {
      type: 'step/start', seq: 2, time: now.getTime() - 1000, data: { turn: 1, step: 1 }
    },
    {
      type: 'assistant/message', seq: 3, time: now.getTime(),
      data: {
        turn: 1, step: 1,
        message: { id: 'dsh-call', source: { provider: 'deepseek', model: 'deepseek-reasoner' } },
        usage: { inputTokens: 2885, outputTokens: 25, reasoningTokens: 23, cacheReadTokens: 0 },
        stream: [{ time0: now.getTime() - 750, time: now.getTime() - 700, chunk: { type: 'reasoning-delta' } }]
      }
    }
  ];
  const text = rows.map(row => JSON.stringify(row)).join('\n') + '\n';
  writeFileSync(join(dir, 'session.v3.jsonl.zstd'), zstdCompressSync(Buffer.from(text)));
}

const live = writeCodex(now, 'warm-write');
for (let index = 1; index <= 102; index++)
{
  writeCodex(new Date(now.getTime() - index), String(index));
}
writeCodex(older, 'older');
writeAntigravity('antigravity-cli');
writeAntigravity('antigravity');
writeDsh();
const routeDir = join(root, '.cc-switch');
mkdirSync(routeDir);
const router = new DatabaseSync(join(routeDir, 'cc-switch.db'));
router.exec('CREATE TABLE proxy_request_logs (request_id TEXT, provider_id TEXT, app_type TEXT, '
  + 'output_tokens INTEGER, latency_ms INTEGER, first_token_ms INTEGER, data_source TEXT, session_id TEXT, created_at INTEGER)');
router.prepare('INSERT INTO proxy_request_logs VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)').run(
  'session:codex:fixture:' + routedRequestId, 'fixture', 'codex', 210, 1250, 250, 'proxy', 'fixture-route', Math.floor(now.getTime() / 1000)
);
router.close();
console.log(JSON.stringify({ home: root, warmWrite: live, expectedToday: 103 * 110 + 400 + 2 * 41 + 2910, expectedTotal: 104 * 110 + 400 + 2 * 41 + 2910 }));
