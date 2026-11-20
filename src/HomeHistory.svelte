<script lang="ts">
  import type { Activity } from './api';
  import { compact, icons, local, number } from './display';

  let { data, failed }: { data: Activity | null; failed: boolean } = $props();
  const weeks = 33;
  const oneDay = 86_400_000;
  const weekColumns = 'repeat(' + weeks + ', minmax(0, 1fr))';
  const month = new Intl.DateTimeFormat(navigator.language, { month: 'short', timeZone: 'UTC' });
  const day = new Intl.DateTimeFormat(navigator.language, { month: 'numeric', day: 'numeric', timeZone: 'UTC' });

  const toDay = (value: string) => Date.parse(value + 'T00:00:00Z');
  const dateKey = (at: number) => new Date(at).toISOString().slice(0, 10);
  let history = $derived.by(() => {
    if (!data) return null;
    const finish = toDay(data.today);
    const sunday = finish + (6 - (new Date(finish).getUTCDay() + 6) % 7) * oneDay;
    const first = sunday - (weeks * 7 - 1) * oneDay;
    const values = new Map(data.days.map(row => [row.date, row.tokens]));
    values.set(data.today, data.todayTokens);
    const cells = Array.from({ length: weeks * 7 }, (_, index) => {
      const at = first + index * oneDay;
      const date = dateKey(at);
      return { date, at, tokens: values.get(date) ?? 0, future: at > finish };
    });
    const count = cells.filter(cell => cell.tokens > 0).length;
    const peak = Math.max(1, ...cells.map(cell => cell.tokens));
    const months = Array.from({ length: weeks }, (_, column) => cells[column * 7].at).filter((at, column, starts) =>
      !column || new Date(at).getUTCMonth() !== new Date(starts[column - 1]).getUTCMonth());
    const cutoff = finish - 65 * oneDay;
    const trend = cells.filter(cell => cell.at >= cutoff && !cell.future);
    return { cells, count, peak, months, trend, trendPeak: Math.max(0, ...trend.map(cell => cell.tokens)) };
  });

  let points = $derived(history?.trend.map((row, index, all) => ({
    x: 2 + index / Math.max(1, all.length - 1) * 356,
    y: 53 - row.tokens / Math.max(1, history?.trendPeak ?? 0) * 49
  })) ?? []);
  let line = $derived.by(() => {
    if (points.length < 2) return '';
    let path = `M${points[0].x} ${points[0].y}`;
    for (let index = 1; index < points.length - 1; index++)
    {
      const { x, y } = points[index];
      const next = points[index + 1];
      path += ` Q${x} ${y} ${(x + next.x) / 2} ${(y + next.y) / 2}`;
    }
    const last = points[points.length - 1];
    return path + ` L${last.x} ${last.y}`;
  });
</script>

<section class="home-section">
  <header class="section-heading">
    <h2>{local('Activity', '活动')}</h2>
    <span class="section-meta">
      {#if history}{local(`${history.count} active days`, `活跃 ${history.count} 天`)}{/if}
      <span class="icon" style:--icon={'url(' + icons.trends + ')'} aria-hidden="true"></span>
    </span>
  </header>
  {#if history}
    <div class="activity-grid" style:--activity-columns={weekColumns} role="img"
      aria-label={local(`Daily activity, ${history.count} active days`, `按日活动，活跃 ${history.count} 天`)}>
      {#each history.cells as cell (cell.date)}
        <span data-level={cell.future ? -1 : Math.ceil(4 * Math.sqrt(cell.tokens / history.peak))}
          title={cell.future ? undefined : day.format(cell.at) + ' · ' + number(cell.tokens) + ' Token'}></span>
      {/each}
    </div>
    <div class="activity-months" style:--activity-columns={weekColumns} aria-hidden="true">
      {#each history.months as at (at)}
        <span style:grid-column={Math.round((at - history.cells[0].at) / (oneDay * 7)) + 1}>{month.format(at)}</span>
      {/each}
    </div>
  {:else}
    <p class="notice">{failed ? local('Could not read activity history.', '无法读取活动记录。') :
      local('Reading activity…', '正在读取活动记录…')}</p>
  {/if}
</section>

{#if history}
  <section class="home-section trend-section">
    <header class="section-heading">
      <h2>{local('Trend', '趋势')}</h2>
      <span class="section-meta">{local('Peak ', '峰值 ')}{compact(history.trendPeak)}</span>
    </header>
    <svg class="trend-chart" viewBox="0 0 360 57" preserveAspectRatio="none" role="img"
      aria-label={local(`Daily token trend, peak ${number(history.trendPeak)}`, `每日 Token 趋势，峰值 ${number(history.trendPeak)}`)}>
      <path d={line} fill="none" stroke="currentColor" stroke-width="2.4" vector-effect="non-scaling-stroke" stroke-linecap="round" />
    </svg>
    <div class="trend-dates" aria-hidden="true">
      <span>{day.format(history.trend[0].at)}</span>
      <span>{day.format(history.trend[Math.floor(history.trend.length / 2)].at)}</span>
      <span>{day.format(history.trend[history.trend.length - 1].at)}</span>
    </div>
  </section>
{/if}
