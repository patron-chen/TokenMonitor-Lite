<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { backend, toolIds } from './api';
  import type { QuotaBalance, QuotaHistory, QuotaKey, Quotas, QuotaWindow, ToolQuota, ToolSelection } from './api';
  import { apiDollars, compact, local, relativeTime, time, toolInfo } from './display';
  import { duration, forecastLabel, quotaGroups, quotaMoney, remainingPercent, reset } from './quota-display';
  import QuotaBanked from './QuotaBanked.svelte';
  import QuotaHistoryCard from './QuotaHistoryCard.svelte';

  interface Props { data: Quotas | null; loading: boolean; now: number; focusTool: string | null; tracked: ToolSelection }
  interface Metric { key: QuotaKey; label: string; current: string; resetAt: number | null }

  let { data, loading, now, focusTool, tracked }: Props = $props();
  let providers = $derived(toolIds.flatMap(tool => data?.tools.find(row => row.tool === tool) ?? []));
  let element: HTMLElement | undefined = $state();
  let card: HTMLDivElement | undefined = $state();
  let active = $state<{ metric: Metric; anchor: HTMLButtonElement; forecast: boolean } | null>(null);
  let history = $state<QuotaHistory | null>(null);
  let failed = $state(false);
  let pinned = false;
  let hoverBlocked = false;
  let opening: ReturnType<typeof setTimeout> | undefined;
  let closing: ReturnType<typeof setTimeout> | undefined;
  let alignedTool: string | null = null;
  const remembered = new Map<string, { revision: number | null; history: QuotaHistory }>();
  let visibleMetric = $derived(active && currentMetric(active.metric));
  let activeKey = $derived(active && !active.forecast ? JSON.stringify(active.metric.key) : '');
  let activeWindow = $derived.by(() => {
    const key = active?.metric.key;
    if (!active?.forecast || key?.kind !== 'window') return null;
    return providers.find(row => row.tool === key.tool)?.windows.find(row => row.group === key.group && row.minutes === key.minutes);
  });
  let forecast = $derived(active && activeWindow?.resetsAt && activeWindow.resetsAt > now && tracked[active.metric.key.tool]
    ? activeWindow.forecast : null);

  onMount(() => {
    const viewport = element?.closest('.viewport');
    const updatePosition = () => {
      if (!active || !viewport) return;
      const row = active.anchor.getBoundingClientRect();
      const clip = viewport.getBoundingClientRect();
      if (row.bottom <= clip.top || row.top >= clip.bottom) dismiss();
      else position();
    };
    viewport?.addEventListener('scroll', updatePosition, { passive: true });
    const size = new ResizeObserver(position);
    if (card) size.observe(card);
    return () => {
      viewport?.removeEventListener('scroll', updatePosition);
      size.disconnect();
      clearTimeout(opening);
      clearTimeout(closing);
    };
  });

  $effect(() => {
    const tool = focusTool;
    if (!element || !tool || tool === alignedTool || !providers.some(provider => provider.tool === tool)) return;
    const header = element.querySelector('[data-quota-tool="' + CSS.escape(tool) + '"]');
    if (header)
    {
      alignedTool = tool;
      header.scrollIntoView({ block: 'nearest' });
    }
  });

  $effect(() => {
    const key = active?.forecast ? null : active?.metric.key;
    const revision = data ? Math.max(data.checkedAt ?? 0, data.observedAt) : null;
    if (!key) return;
    const id = JSON.stringify(key);
    const previous = remembered.get(id);
    history = previous?.history ?? null;
    failed = false;
    if (previous?.revision === revision) return;
    let cancelled = false;
    backend.quotaHistory(key).then(result => {
      if (cancelled) return;
      remembered.set(id, { revision, history: result });
      history = result;
    }).catch(() => {
      if (!cancelled && !previous) failed = true;
    });
    return () => { cancelled = true; };
  });

  function source(provider: ToolQuota)
  {
    if (provider.tool === 'antigravity') return 'RPC';
    if (provider.tool === 'dsh') return 'API';
    return local('Local logs', '本地日志') + (provider.bankedReset ? ' · OAuth' : '');
  }

  function windowMetric(provider: ToolQuota, window: QuotaWindow): Metric
  {
    const remaining = remainingPercent(window.remaining);
    return {
      key: { kind: 'window', tool: provider.tool, group: window.group, minutes: window.minutes },
      label: [window.group, duration(window)].filter(Boolean).join(' · ') || local('Quota', '额度'),
      current: local(remaining + ' left', '剩余 ' + remaining),
      resetAt: window.resetsAt
    };
  }

  function balanceMetric(provider: ToolQuota, balance: QuotaBalance): Metric
  {
    return {
      key: { kind: 'balance', tool: provider.tool, currency: balance.currency },
      label: local('API balance', 'API 余额') + ' · ' + balance.currency,
      current: quotaMoney(balance.amount, balance.currency),
      resetAt: null
    };
  }

  function currentMetric(metric: Metric)
  {
    const { key } = metric;
    const provider = providers.find(row => row.tool === key.tool);
    if (!provider) return metric;
    if (key.kind === 'balance')
    {
      const balance = provider.balances.find(row => row.currency === key.currency);
      return balance ? balanceMetric(provider, balance) : metric;
    }
    const window = provider.windows.find(row => row.group === key.group && row.minutes === key.minutes);
    return window ? windowMetric(provider, window) : metric;
  }

  async function open(metric: Metric, anchor: HTMLButtonElement, forecast = false)
  {
    active = { metric, anchor, forecast };
    await tick();
    if (!card || active?.anchor !== anchor) return;
    if (!card.matches(':popover-open')) card.showPopover();
    position();
  }

  function position()
  {
    if (!active || !card?.matches(':popover-open')) return;
    // Leave the forecast button reachable when the history card opens below its reading.
    const anchor = active.forecast ? active.anchor : active.anchor.closest('.limits-window') ?? active.anchor;
    const row = anchor.getBoundingClientRect();
    const panel = card.getBoundingClientRect();
    const space = 8;
    const right = window.innerWidth - panel.width - space;
    const bottom = window.innerHeight - panel.height - space;
    if (active.forecast)
    {
      const above = row.top - panel.height - space;
      card.style.left = Math.max(space, Math.min(right, row.right - panel.width)) + 'px';
      card.style.top = Math.max(space, Math.min(bottom, above >= space ? above : row.bottom + space)) + 'px';
      return;
    }
    const besideRight = row.right + space <= right;
    const besideLeft = row.left - panel.width - space >= space;
    const left = besideRight ? row.right + space : besideLeft ? row.left - panel.width - space : row.left;
    const above = row.top - panel.height - space;
    const below = row.bottom + space;
    const vertical = below <= bottom || above < space ? below : above;
    const top = besideRight || besideLeft ? row.top : vertical;
    card.style.left = Math.max(space, Math.min(right, left)) + 'px';
    card.style.top = Math.max(space, Math.min(bottom, top)) + 'px';
  }

  function cancelTimers()
  {
    clearTimeout(opening);
    clearTimeout(closing);
  }

  function hover(metric: Metric, anchor: HTMLButtonElement, delay = 180)
  {
    cancelTimers();
    if (pinned || active?.anchor === anchor) return;
    opening = setTimeout(() => void open(metric, anchor), delay);
  }

  function pointerReading(event: PointerEvent & { currentTarget: HTMLButtonElement }, metric: Metric)
  {
    if (event.pointerType !== 'mouse') return;
    if (event.type === 'pointermove')
    {
      if (!hoverBlocked) return;
      hoverBlocked = false;
    }
    if (!hoverBlocked) hover(metric, event.currentTarget);
  }

  function leave()
  {
    cancelTimers();
    if (!pinned) closing = setTimeout(dismiss, 170);
  }

  function select(metric: Metric, anchor: HTMLButtonElement)
  {
    if (pinned && active?.anchor === anchor) { dismiss(); return; }
    cancelTimers();
    pinned = true;
    void open(metric, anchor);
  }

  function showForecast(metric: Metric, anchor: HTMLButtonElement)
  {
    cancelTimers();
    pinned = false;
    void open(metric, anchor, true);
  }

  function dismiss()
  {
    cancelTimers();
    if (card?.matches(':popover-open')) card.hidePopover();
    active = null;
    pinned = false;
  }

  function outside(event: PointerEvent)
  {
    if (active && event.target instanceof Node && !active.anchor.contains(event.target) && !card?.contains(event.target)) dismiss();
  }

  function escape(event: KeyboardEvent)
  {
    if (event.key !== 'Escape' || !active) return;
    event.preventDefault();
    const anchor = active.anchor;
    const restore = card?.contains(document.activeElement);
    // Removing the popover can expose another quota under a stationary pointer.
    hoverBlocked = true;
    dismiss();
    if (restore)
    {
      anchor.focus({ preventScroll: true });
      cancelTimers();
    }
  }
</script>

<svelte:window onresize={position} onpointerdown={outside} onkeydown={escape} />

{#snippet windowRow(provider: ToolQuota, window: QuotaWindow)}
  {@const metric = windowMetric(provider, window)}
  <div class="limits-window">
    <button class="limits-reading" type="button" aria-label={metric.label + ' · ' + metric.current}
      aria-controls="quota-history-card" aria-expanded={activeKey === JSON.stringify(metric.key)}
      onpointerenter={event => pointerReading(event, metric)} onpointermove={event => pointerReading(event, metric)}
      onpointerleave={leave} onfocus={event => hover(metric, event.currentTarget, 0)} onblur={leave}
      onclick={event => select(metric, event.currentTarget)}>
      <span class="limits-reading-line">
        <span>{duration(window) || local('Quota', '额度')}</span><b>{metric.current}</b>
      </span>
      <span class="limits-meter" aria-hidden="true"><span style:--remaining={window.remaining + '%'}></span></span>
      <span class="quota-reset" title={time(window.resetsAt)}>{reset(window.resetsAt, now)}</span>
    </button>
    {#if window.minutes === 300 || window.minutes === 10080}
      <div class="limits-forecast">
        <span>{forecastLabel(window, tracked[provider.tool], now)}</span>
        <button class="quota-banked-info forecast-info" type="button"
          aria-label={metric.label + ' · ' + local('Usage this cycle', '本周期已用量')}
          aria-describedby={active?.forecast && active.metric.label === metric.label && active.metric.key.tool === provider.tool ? 'quota-history-card' : undefined}
          onpointerenter={event => showForecast(metric, event.currentTarget)} onpointerleave={leave}
          onfocus={event => showForecast(metric, event.currentTarget)} onblur={leave}
          onclick={event => showForecast(metric, event.currentTarget)}>!</button>
      </div>
    {/if}
  </div>
{/snippet}

<section class="limits-view" bind:this={element} aria-label={local('Tool limits', '工具额度')} aria-busy={loading}>
  {#each providers as provider (provider.tool)}
    {@const tool = toolInfo(provider.tool)}
    <article class="limits-provider" data-quota-tool={provider.tool} style:--row-ink={tool.color}>
      <header class="limits-provider-head">
        <span class="row-name">
          <span class="icon" style:--icon={'url(' + tool.icon + ')'} aria-hidden="true"></span>
          <span class="ellipsis">{tool.name}</span>
        </span>
        {#if provider.plan}<span class="row-meta ellipsis">{provider.plan}</span>{/if}
      </header>
      <p class="limits-provider-meta" title={time(provider.observedAt)}>
        {local('Updated ', '更新于 ')}{relativeTime(provider.observedAt, now)} · {source(provider)}
      </p>
      {#each Array.from(quotaGroups(provider.windows)) as [group, windows] (group)}
        <div class="limits-group">
          {#if group}<h3>{group}</h3>{/if}
          <div class="limits-grid">
            {#each windows as window}
              {@render windowRow(provider, window)}
            {/each}
          </div>
        </div>
      {/each}
      {#each provider.balances as balance (balance.currency)}
        {@const metric = balanceMetric(provider, balance)}
        <button class="limits-reading limits-balance" type="button" aria-label={metric.label + ' · ' + metric.current}
          aria-controls="quota-history-card" aria-expanded={activeKey === JSON.stringify(metric.key)}
          onpointerenter={event => pointerReading(event, metric)} onpointermove={event => pointerReading(event, metric)}
          onpointerleave={leave} onfocus={event => hover(metric, event.currentTarget, 0)} onblur={leave}
          onclick={event => select(metric, event.currentTarget)}>
          <span class="limits-reading-line"><span>{local('API balance', 'API 余额')}</span><b>{metric.current}</b></span>
        </button>
      {/each}
      <QuotaBanked bank={provider.bankedReset} {now} />
    </article>
  {:else}
    <p class="notice" role="status">{loading ? local('Reading tool quotas…', '正在读取工具额度…') :
      local('No tool quota available yet.', '暂无可读的工具额度。')}</p>
  {/each}
</section>

<div id="quota-history-card" class="quota-history-card" class:forecast-tip={active?.forecast} popover="manual" bind:this={card}
  role={active?.forecast ? 'tooltip' : 'region'} aria-label={active?.forecast ? local('Usage this cycle', '本周期已用量') : local('Quota history', '额度使用记录')}
  style:--row-ink={visibleMetric ? toolInfo(visibleMetric.key.tool).color : undefined}
  onpointerenter={cancelTimers} onpointerleave={leave} onfocusin={cancelTimers}>
  {#if active?.forecast && activeWindow}
    {#if forecast}
      <strong>{local('Used locally this cycle', '本周期本机已用')}</strong>
      {#if forecast.usedTokens == null}
        <div>{local('No local reading this cycle', '本周期暂无本机记录')}</div>
      {:else}
        <div>{compact(forecast.usedTokens)} Token ·
          {forecast.usedUsd == null ? local('Price unavailable', '暂无估价') : apiDollars(forecast.usedUsd)}
          {forecast.missingPrice && forecast.usedUsd != null ? local(' priced subtotal', ' 已计价小计') : ''}</div>
      {/if}
      <small>{local('API-equivalent USD', 'API 等值美元')} ·
        {forecast.basis === 'history' ? local('Recent history', '近期历史参考') :
          forecast.basis === 'current' ? local('Current cycle', '本期推算') : local('Collecting samples', '样本不足')}</small>
      {#if forecast.partial}<small>{local('Partial local records', '本机记录不完整')}</small>{/if}
      <small>{local('As of ', '截至 ')}{time(forecast.observedAt)}</small>
    {:else}
      {forecastLabel(activeWindow, tracked[active.metric.key.tool], now)}
    {/if}
  {:else if visibleMetric}
    {#key JSON.stringify(visibleMetric.key)}
      <QuotaHistoryCard name={toolInfo(visibleMetric.key.tool).name} {...visibleMetric} {history} {failed} />
    {/key}
  {/if}
</div>
