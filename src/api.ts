import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type Period = 'today' | 'month' | 'week' | 'last7' | 'last30' | 'allTime';
export type StatisticsRange = Extract<Period, 'week' | 'last7' | 'last30'>;
export type View = 'home' | 'limits' | 'tools' | 'models' | 'sessions';
export type Sort = 'newest' | 'tokens';
export type ToolMode = 'tokens' | 'models';
export type Backdrop = 'none' | 'system' | 'accent';
export const toolIds = ['codex', 'antigravity', 'dsh'] as const;
export type ToolId = typeof toolIds[number];
export type ToolSelection = Record<ToolId, boolean>;
export type Cadence = 'live' | '5m' | '15m' | '30m';
export type UiLanguage = 'en' | 'zh-CN';
export type Language = 'system' | UiLanguage;
export type SettingsSection = 'appearance' | 'collection' | 'quotas';

export interface Preferences
{
  language: Language;
  statisticsRange: StatisticsRange;
  collection: { cadence: Cadence; tools: ToolSelection };
  quotas: { refreshMinutes: number; tools: ToolSelection };
}

export interface CollectionStatus
{
  timezone: string;
  updatedAt: number | null;
  scanning: boolean;
  failure: string | null;
  watching: boolean;
  sources: { tool: ToolId; tokens: number | null; roots: { path: string; exists: boolean }[] }[];
}

export const defaultPreferences: Preferences = {
  language: 'system',
  statisticsRange: 'last30',
  collection: { cadence: 'live', tools: { codex: true, antigravity: true, dsh: true } },
  quotas: { refreshMinutes: 2, tools: { codex: true, antigravity: true, dsh: true } }
};

export interface Counts
{
  input: number | null;
  output: number | null;
  cacheRead: number | null;
  cacheWrite: number | null;
  reasoning: number | null;
}

export interface Usage
{
  counts: Counts;
  tokens: number;
  costUsd: number | null;
  missingPrice: boolean;
}

export interface Breakdown
{
  key: string;
  usage: Usage;
  models: Breakdown[];
}

export interface Summary
{
  updatedAt: number | null;
  usage: Usage;
  tools: Breakdown[];
  models: Breakdown[];
  refreshing: boolean;
  failure: string | null;
  antigravityLocal: boolean;
  antigravityPbOnly: boolean;
}

export interface DailyUsage { date: string; tokens: number }
export interface Activity { today: string; todayTokens: number; days: DailyUsage[] }

export interface QuotaWindow
{
  group: string;
  minutes: number | null;
  remaining: number;
  resetsAt: number | null;
}

export interface QuotaBalance { currency: 'USD' | 'CNY'; amount: number }

export interface ToolQuota
{
  tool: ToolId;
  plan: string | null;
  observedAt: number;
  windows: QuotaWindow[];
  balances: QuotaBalance[];
  bankedReset: { availableCount: number; expirations: number[] } | null;
}

export interface Quotas { checkedAt: number | null; observedAt: number; tools: ToolQuota[] }

export type QuotaKey =
  | { kind: 'window'; tool: ToolId; group: string; minutes: number | null }
  | { kind: 'balance'; tool: ToolId; currency: QuotaBalance['currency'] };

export interface QuotaHistory
{
  start: number;
  end: number;
  cycle: boolean;
  observedAt: number | null;
  trend: { at: number; value: number; split: boolean }[];
  days: { date: string; used: number | null }[];
  unallocated: number;
}

export interface SessionKey { tool: string; id: string }

export interface SessionMetrics
{
  startedAt: number | null;
  workspace: string | null;
  calls: number | null;
  tokensPerSecond: number | null;
  subagents: number;
  backgroundReview: boolean;
}

export interface Session extends SessionKey
{
  title: string;
  models: string[];
  modelUsage: { model: string; tokens: number }[];
  lastUsedAt: number | null;
  usage: Usage;
  metrics: SessionMetrics;
  nativeId: string | null;
  reasoningEffort: string | null;
  latestModel: string | null;
  archived: boolean;
  activity: { state: 'running' | 'waiting' | 'ended'; at: number } | null;
  context: { tokens: number; window: number; at: number } | null;
}

export interface SessionPage
{
  rows: Session[];
  total: number;
  tools: string[];
}

export interface Turn
{
  at: number | null;
  model: string | null;
  tools: string[];
  counts: Counts;
  tokens: number;
  kind: 'reply' | 'attempt' | 'compaction';
  reasoningEffort: string | null;
  performance: RequestPerformance | null;
}

export interface RequestPerformance
{
  source: 'routed' | 'transcript';
  tokensPerSecond: number | null;
  firstTokenMs: number | null;
  hiddenReasoning: boolean;
}

export interface Exchange
{
  id: number;
  prompt: string | null;
  promptAt: number | null;
  lastUsedAt: number | null;
  tokens: number;
  turns: Turn[];
}

export interface Detail
{
  session: Session;
  exchanges: Exchange[];
  total: number;
  status: 'missing' | 'unreadable' | 'partial' | 'ready';
  observedTokens: number;
}

export interface ViewState
{
  period: Period;
  view: View;
  fromHome: boolean;
  tool: string | null;
  query: string;
  searchOpen: boolean;
  page: number;
  scroll: number;
  selection: SessionKey | null;
  sort: Sort;
  detailPage: number;
  detailScroll: number;
  openExchanges: number[];
  expanded: string | null;
  toolMode: ToolMode;
}

export interface Appearance
{
  opacity: number;
  fontScale: number;
}

export const pageSize = 100;
export const emptySessions: SessionPage = { rows: [], total: 0, tools: [] };

export const backend = {
  detailsReady: () => invoke<void>('details_ready'),
  view: () => invoke<{
    view: ViewState | null; backdrop: Backdrop; appearance: Appearance; preferences: Preferences; cachedQuotas: Quotas | null;
    systemLanguage: UiLanguage;
  }>('get_view_state'),
  remember: (view: ViewState) => invoke<void>('remember_view', { view }),
  appearance: (appearance: Appearance) => invoke<void>('set_appearance', { appearance }),
  preferences: (preferences: Preferences, period: Period) => invoke<void>('set_preferences', { preferences, period }),
  collection: () => invoke<CollectionStatus>('get_collection_status'),
  dismiss: () => invoke<void>('dismiss_window'),
  summary: (period: Period) => invoke<Summary>('get_summary', { period }),
  activity: (force: boolean) => invoke<Activity>('get_activity', { force }),
  quotas: (force: boolean) => invoke<Quotas>('get_quotas', { force }),
  quotaHistory: (key: QuotaKey) => invoke<QuotaHistory>('get_quota_history', { key }),
  sessions: (period: Period, tool: string | null, query: string, page: number, preview: boolean) =>
    invoke<SessionPage>('list_sessions', { period, tool, query, page, preview }),
  detail: (period: Period, selection: SessionKey, sort: Sort, page: number) =>
    invoke<Detail | null>('get_session_detail', { period, tool: selection.tool, sessionId: selection.id, sort, page }),
  refresh: (period: Period) => invoke<void>('refresh', { period }),
  updated: (callback: (period: Period) => void) =>
    listen<{ period: Period; revision: number }>('usage-updated', event => callback(event.payload.period))
};
