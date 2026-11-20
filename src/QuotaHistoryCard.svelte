<script lang="ts">
  import type { QuotaHistory, QuotaKey } from './api';
  import { local, time } from './display';
  import { quotaMoney } from './quota-display';

  interface Props
  {
    name: string;
    label: string;
    current: string;
    key: QuotaKey;
    resetAt: number | null;
    history: QuotaHistory | null;
    failed: boolean;
  }

  let { name, label, current, key, resetAt, history, failed }: Props = $props();
  let hoveredDay = $state<string | null>(null);
  const dayFormat = new Intl.DateTimeFormat(navigator.language, { month: 'numeric', day: 'numeric' });
  const hourFormat = new Intl.DateTimeFormat(navigator.language, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
  const percent = new Intl.NumberFormat(navigator.language, { style: 'percent', maximumFractionDigits: 1 });
  const width = 240;
  const height = 65;
  let min = $derived(key.kind === 'window' ? 0 : Math.min(0, ...(history?.trend.map(point => point.value) ?? [])));
  let max = $derived(key.kind === 'window' ? 100 : Math.max(1, ...(history?.trend.map(point => point.value) ?? [])));
  let peak = $derived(Math.max(0.001, ...(history?.days.map(day => day.used ?? 0) ?? [])));
  let latestDay = $derived(history?.days[history.days.length - 1]);
  let reading = $derived(history?.days.find(day => day.date === hoveredDay) ?? latestDay);
  let trendFormat = $derived(history && history.end - history.start < 86_400_000 ? hourFormat : dayFormat);

  function format(value: number)
  {
    return key.kind === 'window' ? percent.format(value / 100) : quotaMoney(value, key.currency);
  }

  function dayLabel(date: string)
  {
    return dayFormat.format(new Date(date + 'T00:00:00'));
  }

  let plotted = $derived(history?.trend.map(point => ({
    split: point.split,
    x: 2 + (point.at - history!.start) / Math.max(1, history!.end - history!.start) * (width - 4),
    y: 2 + (max - point.value) / (max - min) * (height - 4)
  })) ?? []);
  let path = $derived(plotted.map(point =>
    (point.split ? 'M' : 'L') + point.x.toFixed(2) + ' ' + point.y.toFixed(2)).join(' '));
</script>

<header class="quota-history-title">
  <strong class="ellipsis" title={name}>{name}</strong>
  {#if history?.observedAt}<time>{time(history.observedAt)}</time>{/if}
</header>
<div class="quota-history-current">
  <span class="ellipsis" title={label}>{label}</span><b>{current}</b>
</div>

{#if failed}
  <p class="quota-history-message" role="status">{local('Could not read quota history.', '无法读取额度记录。')}</p>
{:else if !history}
  <p class="quota-history-message" role="status">{local('Reading quota history…', '正在读取额度记录…')}</p>
{:else}
  <div class="quota-history-heading">
    <h3>{key.kind === 'window' ? local('Remaining trend', '剩余走势') : local('Balance trend', '余额走势')}</h3>
    {#if !history.cycle}<span>{local('Last 14 days', '近 14 天')}</span>{/if}
  </div>
  {#if plotted.length}
    <div class="quota-history-graph">
      <div class="quota-history-y"><span>{format(max)}</span><span>{format(min)}</span></div>
      <svg viewBox={'0 0 ' + width + ' ' + height} preserveAspectRatio="none" role="img"
        aria-label={key.kind === 'window' ? local('Observed remaining quota', '实测剩余额度') : local('Observed API balance', '实测 API 余额')}>
        <path class="quota-history-grid" d={'M2 2H238 M2 32.5H238 M2 63H238'} />
        {#if history.cycle}
          <path class="quota-history-guide" d="M2 2L238 63">
            <title>{local('Even-use reference', '均匀使用参考线')}</title>
          </path>
        {/if}
        <path class="quota-history-curve" d={path} />
        {#each plotted as point, index (index)}
          {#if point.split && plotted[index + 1]?.split !== false}
            <circle class="quota-history-point" cx={point.x} cy={point.y} r="2" />
          {/if}
        {/each}
      </svg>
      <div class="quota-history-axis">
        <span>{trendFormat.format(history.start)}</span><span>{trendFormat.format(history.end)}</span>
      </div>
    </div>
  {:else}
    <p class="quota-history-message">{local('No observations yet.', '暂无观测记录。')}</p>
  {/if}

  <section class="quota-history-daily" aria-label={local('Daily quota usage', '每日额度用量')}>
    <div class="quota-history-heading">
      <h3>{local('Daily quota usage', '每日额度用量')}</h3><span>{local('Last 14 days', '近 14 天')}</span>
    </div>
    <div class="quota-history-bars">
      {#each history.days as day (day.date)}
        {@const amount = day.used == null ? local('No measured change', '暂无可计量数据') : format(day.used)}
        <button class:active={hoveredDay === day.date} type="button"
          title={dayLabel(day.date) + ' · ' + amount} aria-label={dayLabel(day.date) + ' · ' + amount}
          onpointerenter={() => { hoveredDay = day.date; }} onpointerleave={() => { hoveredDay = null; }}
          onfocus={() => { hoveredDay = day.date; }} onblur={() => { hoveredDay = null; }}>
          {#if day.used != null}<span class="quota-history-bar" style:--height={(day.used / peak * 100) + '%'}></span>{/if}
        </button>
      {/each}
    </div>
    {#if latestDay}
      <div class="quota-history-axis"><span>{dayLabel(history.days[0].date)}</span><span>{dayLabel(latestDay.date)}</span></div>
    {/if}
    {#if reading}
      <p class="quota-history-reading">
        {hoveredDay ? dayLabel(reading.date) : local('Today', '今日')} ·
        <b>{reading.used == null ? local('No measured change', '暂无可计量数据') : format(reading.used)}</b>
      </p>
    {/if}
  </section>
  {#if resetAt || history.unallocated > 0}
    <footer class="quota-history-footer">
      {#if resetAt}<div>{local('Resets at', '重置于')} {time(resetAt)}</div>{/if}
      {#if history.unallocated > 0}<div>{format(history.unallocated)} {local('spans multiple days', '的用量跨日，未分配到单日')}</div>{/if}
    </footer>
  {/if}
{/if}
