import type { Counts, RequestPerformance, Session, Usage, View } from './api';
import codex from './icons/providers/codex.svg?url';
import antigravity from './icons/providers/antigravity.svg?url';
import dsh from './icons/providers/dsh.svg?url';
import home from './icons/views/home.svg?url';
import tools from './icons/views/tool.svg?url';
import models from './icons/views/model.svg?url';
import sessions from './icons/views/session.svg?url';
import limits from './icons/views/limits.svg?url';
import trends from './icons/views/trends.svg?url';
import refresh from './icons/actions/refresh.svg?url';
import settings from './icons/actions/settings.svg?url';
import appearance from './icons/settings/appearance.svg?url';
import collection from './icons/settings/collection.svg?url';
import search from './icons/actions/search.svg?url';
import back from './icons/actions/arrow-left.svg?url';
import spinner from './icons/actions/spinner.svg?url';
import gemini from './icons/providers/gemini.svg?url';

export const zh = navigator.language.toLowerCase().startsWith('zh');
export const local = (english: string, chinese: string) => zh ? chinese : english;
export const icons = { home, tools, models, sessions, limits, trends, refresh, settings, appearance, collection, search, back, spinner };
export const views: View[] = ['home', 'limits', 'tools', 'models', 'sessions'];
export const viewLabel: Record<View, string> = {
  home: local('Home', '主页'),
  limits: local('Limits', '额度'),
  tools: local('Tools', '工具'),
  models: local('Models', '模型'),
  sessions: local('Sessions', '会话')
};

const clients: Record<string, { name: string; icon: string; color: string }> = {
  codex: { name: 'Codex', icon: codex, color: '#54b8c5' },
  antigravity: { name: 'Antigravity', icon: antigravity, color: '#438af5' },
  dsh: { name: 'DeepSeek Harness', icon: dsh, color: '#818afa' }
};

export const toolInfo = (key: string) => clients[key] ?? { name: key, icon: '', color: '#b7ead4' };
export const modelIcon = (model: string, tool: string) => /^gemini[- ]/i.test(model) ? gemini : toolInfo(tool).icon;
export const number = new Intl.NumberFormat(navigator.language).format;
export const compact = new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 }).format;
const date = new Intl.DateTimeFormat(navigator.language, {
  month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit'
});
export const time = (at: number | null) => at ? date.format(at) : '';
const clock = new Intl.DateTimeFormat(navigator.language, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
const calendar = new Intl.DateTimeFormat(navigator.language, { month: 'numeric', day: 'numeric' });
const relative = new Intl.RelativeTimeFormat(navigator.language, { numeric: 'always' });

export function shortSessionTime(at: number | null, now: number): string
{
  if (at == null) return '';
  return new Date(at).toDateString() === new Date(now).toDateString()
    ? clock.format(at) : calendar.format(at) + ' ' + clock.format(at);
}

export function relativeTime(at: number | null, now: number): string
{
  if (!at) return '';
  const minutes = Math.round(Math.max(0, now - at) / 60_000);
  if (!minutes) return local('just now', '刚刚');
  if (minutes < 60) return local(relative.format(-minutes, 'minute'), `${number(minutes)} 分钟前`);
  const hours = Math.round(minutes / 60);
  if (hours < 24) return local(relative.format(-hours, 'hour'), `${number(hours)} 小时前`);
  const days = Math.round(hours / 24);
  return local(relative.format(-days, 'day'), `${number(days)} 天前`);
}

export function cost(usage: Usage, precise = false): string
{
  if (usage.costUsd == null)
  {
    return usage.missingPrice ? local('Price unavailable', '暂无估价') : '—';
  }
  const amount = usage.costUsd;
  const digits = amount === 0 || (!precise && amount >= 1) ? 2 : 4;
  const formatted = amount > 0 && amount < 0.0001 ? '<$0.0001' : '$' + amount.toFixed(digits);
  return usage.missingPrice ? formatted + local(' priced subtotal', ' 已计价小计') : formatted;
}

const splitLabels: Record<keyof Counts, string> = {
  input: local('Input (cache miss)', '输入（未命中）'),
  output: local('Output', '输出'),
  cacheRead: local('Cache read', '缓存读取'),
  cacheWrite: local('Cache write', '缓存写入'),
  reasoning: local('Reasoning', '推理')
};

export function splits(counts: Counts, format = number)
{
  return (Object.keys(splitLabels) as (keyof Counts)[])
    .filter(key => counts[key] != null)
    .map(key => ({ key, label: splitLabels[key], value: format(counts[key] ?? 0) }));
}

export function sessionTitle(session: Session): string
{
  if (session.title) return session.title;
  if (session.metrics.backgroundReview) return local('Guardian review', '后台审查');
  return session.metrics.workspace || (session.nativeId ?? session.id).slice(0, 12) || '—';
}

export function sessionModelLabel(session: Session): string
{
  const models = session.models.filter(model => model && model !== 'unknown');
  if (session.latestModel)
  {
    const others = models.filter(model => model !== session.latestModel).length;
    return session.latestModel + (others ? ` +${others}` : '');
  }
  return models.length > 1 ? local(`${models.length} models`, `${models.length} 个模型`) : models[0] ?? '';
}

function percentage(value: number): string
{
  return value > 0 && value < 1 ? '<1%' : `${Math.round(Math.min(100, value))}%`;
}

export function usageSplits(counts: Counts)
{
  if (Object.values(counts).every(value => value == null)) return [];

  // The collector provides disjoint buckets. Cache writes are fresh input and reasoning is generated output.
  const hit = counts.cacheRead ?? 0;
  const miss = (counts.input ?? 0) + (counts.cacheWrite ?? 0);
  const output = (counts.output ?? 0) + (counts.reasoning ?? 0);
  const input = hit + miss;
  const share = (value: number) => percentage(input > 0 ? value / input * 100 : 0);
  return [
    { key: 'hit', label: local('Input (Cache Hit)', '输入（缓存命中）'), value: number(hit), share: share(hit) },
    { key: 'miss', label: local('Input (Cache Miss)', '输入（缓存未命中）'), value: number(miss), share: share(miss) },
    { key: 'output', label: splitLabels.output, value: number(output), share: null }
  ];
}

export function sessionModelsTitle(session: Session): string
{
  const label = sessionModelLabel(session);
  if (session.modelUsage.length < 2) return label;
  const total = Math.max(1, session.usage.tokens);
  const rows = session.modelUsage.map(({ model, tokens }) =>
    `${model && model !== 'unknown' ? model : local('Unclassified', '未归类')} · ${compact(tokens)} · ${percentage(tokens / total * 100)}`);
  if (session.latestModel) rows.unshift(local('Last used ', '最后使用 ') + session.latestModel);
  return rows.join('\n');
}

export const effortTitle = (effort: string) => local('Reasoning effort ', '思考等级 ') + effort;
export const outputRateLabel = (rate: number | null) => rate != null && Math.round(rate) > 0 ? number(Math.round(rate)) + ' tok/s' : '';

export function requestPerformanceParts(performance: RequestPerformance | null)
{
  if (!performance) return [];
  const { tokensPerSecond, firstTokenMs, hiddenReasoning } = performance;
  const estimated = performance.source === 'transcript';
  const prefix = estimated ? '≈ ' : '';
  const parts: { value: string; label: string }[] = [];
  if (tokensPerSecond != null)
  {
    const speed = outputRateLabel(tokensPerSecond) || '<1 tok/s';
    const approximateReasoning = !estimated && hiddenReasoning;
    parts.push({
      value: prefix + speed + (approximateReasoning ? local(' (approx.)', '（近似）') : ''),
      label: estimated ? local('Log estimate includes the wait for the first token', '日志估算含首字等待，速率偏低') :
        approximateReasoning ? local('Routed rate · hidden reasoning makes the result approximate', '路由速率 · 思考未外露，速率为近似值') :
          local('Routed generation rate from first token to completion', '路由实测速率，从首字算到结束')
    });
  }
  if (firstTokenMs != null)
  {
    const duration = firstTokenMs < 1000 ? number(firstTokenMs) + ' ms' : (firstTokenMs / 1000).toFixed(1) + ' s';
    parts.push({
      value: local('First token ', '首字 ') + prefix + duration,
      label: estimated ? local('Log estimate includes client request preparation', '从日志时间戳推算，含客户端组装请求的时间') :
        local('Time from router receipt to first upstream output', '从路由服务收到请求到上游开始输出')
    });
  }
  return parts;
}

export function cacheHitLabel(counts: Counts, reported = false): string
{
  const read = counts.cacheRead ?? 0;
  const write = counts.cacheWrite ?? 0;
  const total = (counts.input ?? 0) + read + write;
  return total > 0 && (read > 0 || write > 0 || reported) ? percentage(read / total * 100) : '';
}

export function callLabel(count: number): string
{
  return local(`${number(count)} ${count === 1 ? 'call' : 'calls'}`, `${number(count)} 次调用`);
}

export function sessionActivityParts(session: Session, now: number, view: 'preview' | 'list' | 'detail' = 'list')
{
  const { calls, tokensPerSecond, startedAt } = session.metrics;
  return [
    { value: view !== 'preview' && session.archived ? local('Archived', '已归档') : '', label: local('Session status', '会话状态') },
    { value: view === 'preview' ? '' : shortSessionTime(session.lastUsedAt ?? startedAt, now), label: local('Last used', '最近活动') },
    { value: calls ? callLabel(calls) : '', label: '' },
    { value: cacheHitLabel(session.usage.counts), label: local('Cache hit', '缓存命中') },
    { value: view === 'detail' ? '' : outputRateLabel(tokensPerSecond), label: local('Generation rate', '生成速率') }
  ].filter(part => part.value);
}

export function sessionState(session: Session, now: number): NonNullable<Session['activity']>['state'] | 'idle'
{
  const activity = session.activity;
  return !session.archived && activity && now - activity.at <= 10 * 60_000 ? activity.state : 'idle';
}

export function sessionKey(value: { tool: string; id: string }): string
{
  return value.tool + ':' + value.id;
}
