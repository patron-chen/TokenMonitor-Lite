import type { QuotaWindow, QuotaBalance } from './api';
import { local } from './display';
import { locale, localized } from './language.svelte';

const minutes = localized(locale => new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }));
const percent = localized(locale => new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 0 }));

export function quotaGroups(windows: QuotaWindow[])
{
  const groups = new Map<string, QuotaWindow[]>();
  for (const window of windows)
  {
    const label = window.group.trim();
    const group = groups.get(label);
    if (group) group.push(window);
    else groups.set(label, [window]);
  }
  return groups;
}

export function duration(window: QuotaWindow)
{
  if (window.minutes === 300) return local('5-hour', '5 小时');
  if (window.minutes === 10080) return local('Weekly', '每周');
  return window.minutes ? minutes().format(window.minutes) + local(' min', ' 分钟') : '';
}

export function formatCountdown(total: number)
{
  const days = Math.floor(total / 1440);
  const hours = Math.floor(total % 1440 / 60);
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${total % 60}m`;
  return `${total}m`;
}

export function reset(at: number | null, now: number, compact = false)
{
  if (!at) return '';
  if (at <= now) return local('Awaiting update', '等待更新');
  const countdown = formatCountdown(Math.ceil((at - now) / 60000));
  return compact ? countdown : local(`Reset ${countdown}`, `${countdown} 后重置`);
}

export const remainingPercent = (remaining: number) => percent().format(remaining / 100);

export function quotaMoney(amount: number, currency: QuotaBalance['currency'])
{
  return new Intl.NumberFormat(locale(), {
    style: 'currency', currency, currencyDisplay: 'narrowSymbol'
  }).format(amount);
}
