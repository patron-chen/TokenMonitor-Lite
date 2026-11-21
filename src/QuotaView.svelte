<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { backend, toolIds } from './api';
  import type { QuotaBalance, QuotaKey, Quotas, QuotaWindow, ToolQuota, ToolSelection } from './api';
  import { apiDollars, compact, local, relativeTime, time, toolInfo } from './display';
  import { isChinese } from './language.svelte';
  import { duration, forecastLabel, quotaGroups, quotaMoney, remainingPercent, reset } from './quota-display';
  import { quotaPopup } from './quota-popup';
  import type { QuotaMetric } from './quota-popup';
  import QuotaBanked from './QuotaBanked.svelte';

  interface Props
  {
    data: Quotas | null; loading: boolean; now: number; focusTool: string | null; tracked: ToolSelection; fontScale: number;
    onUpdate: (snapshot: Quotas) => void;
  }

  let { data, loading, now, focusTool, tracked, fontScale, onUpdate }: Props = $props();
  let providers = $derived(toolIds.flatMap(tool => data?.tools.find(row => row.tool === tool) ?? []));
  let element: HTMLElement | undefined = $state();
  let forecastTip: HTMLDivElement | undefined = $state();
  let active = $state<{ metric: QuotaMetric; anchor: HTMLButtonElement; forecast: boolean; id: number; focusPopup: boolean } | null>(null);
  let historyTop = $state<number | null>(null);
  let resetState = $state<Record<string, 'pending' | 'failed'>>({});
  let popupRequested = false;
  let pinned = false;
  let hoverBlocked = false;
  let opening: ReturnType<typeof setTimeout> | undefined;
  let closing: ReturnType<typeof setTimeout> | undefined;
  let alignedTool: string | null = null;
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
    viewport?.addEventListener('scroll', position, { passive: true });
    const size = new ResizeObserver(position);
    if (forecastTip) size.observe(forecastTip);
    if (element) size.observe(element);
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    quotaPopup.onInteraction(({ id, kind }) => {
      if (!active || active.forecast || active.id !== id) return;
      if (kind === 'enter') cancelTimers();
      else if (kind === 'leave') leave();
      else closeAndRestoreFocus();
    }).then(unlisten => {
      if (disposed) unlisten();
      else unsubscribe = unlisten;
    }).catch(() => {});
    return () => {
      disposed = true;
      unsubscribe?.();
      viewport?.removeEventListener('scroll', position);
      size.disconnect();
      cancelTimers();
      clearPopup();
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
    if (!active || active.forecast || historyTop === null)
    {
      clearPopup();
      return;
    }
    const metric = currentMetric(active.metric);
    const revision = data ? Math.max(data.checkedAt ?? 0, data.observedAt) : null;
    const id = active.id;
    popupRequested = true;
    void quotaPopup.set({
      selection: { ...metric, id, revision, language: isChinese() ? 'zh-CN' : 'en', fontScale },
      top: historyTop, focus: active.focusPopup
    }).catch(() => { if (active?.id === id) dismiss(); });
  });

  function clearPopup()
  {
    if (!popupRequested) return;
    popupRequested = false;
    void quotaPopup.set(null).catch(() => {});
  }

  function source(provider: ToolQuota)
  {
    if (provider.tool === 'antigravity') return 'RPC';
    if (provider.tool === 'dsh') return 'API';
    const label = provider.source === 'online' ? local('Online quota', '在线额度') : local('Local records', '本地记录');
    return label + (provider.bankedReset ? ' · OAuth' : '');
  }

  function windowMetric(provider: ToolQuota, window: QuotaWindow): QuotaMetric
  {
    const remaining = remainingPercent(window.remaining);
    return {
      key: { kind: 'window', tool: provider.tool, group: window.group, minutes: window.minutes },
      label: [window.group, duration(window)].filter(Boolean).join(' · ') || local('Quota', '额度'),
      current: local(remaining + ' left', '剩余 ' + remaining),
      resetAt: window.resetsAt
    };
  }

  function balanceMetric(provider: ToolQuota, balance: QuotaBalance): QuotaMetric
  {
    return {
      key: { kind: 'balance', tool: provider.tool, currency: balance.currency },
      label: local('API balance', 'API 余额') + ' · ' + balance.currency,
      current: quotaMoney(balance.amount, balance.currency),
      resetAt: null
    };
  }

  function currentMetric(metric: QuotaMetric)
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

  async function open(metric: QuotaMetric, anchor: HTMLButtonElement, forecast = false, focusPopup = false)
  {
    const id = quotaPopup.nextId();
    active = { metric, anchor, forecast, id, focusPopup };
    if (forecast)
    {
      await tick();
      if (!forecastTip || active?.id !== id) return;
      if (!forecastTip.matches(':popover-open')) forecastTip.showPopover();
    }
    else if (forecastTip?.matches(':popover-open')) forecastTip.hidePopover();
    position();
  }

  function position()
  {
    if (!active) return;
    const clip = element?.closest('.viewport')?.getBoundingClientRect();
    const row = active.anchor.getBoundingClientRect();
    if (!active.anchor.isConnected || (clip && (row.bottom <= clip.top || row.top >= clip.bottom)))
    {
      dismiss();
      return;
    }
    if (!active.forecast)
    {
      historyTop = row.top;
      return;
    }
    if (!forecastTip?.matches(':popover-open')) return;
    const panel = forecastTip.getBoundingClientRect();
    const space = 8;
    const right = window.innerWidth - panel.width - space;
    const bottom = window.innerHeight - panel.height - space;
    const above = row.top - panel.height - space;
    forecastTip.style.left = Math.max(space, Math.min(right, row.right - panel.width)) + 'px';
    forecastTip.style.top = Math.max(space, Math.min(bottom, above >= space ? above : row.bottom + space)) + 'px';
  }

  function cancelTimers()
  {
    clearTimeout(opening);
    clearTimeout(closing);
  }

  function hover(metric: QuotaMetric, anchor: HTMLButtonElement, delay = 180)
  {
    cancelTimers();
    if (pinned || active?.anchor === anchor) return;
    opening = setTimeout(() => void open(metric, anchor), delay);
  }

  function pointerReading(event: PointerEvent & { currentTarget: HTMLButtonElement }, metric: QuotaMetric, forecast = false)
  {
    if (event.pointerType !== 'mouse') return;
    const anchor = event.currentTarget;
    // Keyboard navigation can scroll another quota under a stationary mouse.
    const focusedInfo = document.activeElement !== anchor && document.activeElement?.matches('.forecast-info:focus-visible');
    if (event.type === 'pointermove')
    {
      if ((!hoverBlocked && !focusedInfo) || (!event.movementX && !event.movementY)) return;
      hoverBlocked = false;
    }
    else if (focusedInfo) return;
    if (hoverBlocked) return;
    if (forecast) showForecast(metric, anchor);
    else hover(metric, anchor);
  }

  function leave()
  {
    cancelTimers();
    if (pinned || !active || (active.forecast && active.anchor.matches(':focus-visible'))) return;
    closing = setTimeout(dismiss, active.forecast ? 170 : 600);
  }

  function select(metric: QuotaMetric, event: MouseEvent & { currentTarget: HTMLButtonElement })
  {
    const anchor = event.currentTarget;
    if (pinned && active?.anchor === anchor) { dismiss(); return; }
    cancelTimers();
    pinned = true;
    const keyboard = event.detail === 0;
    if (active?.anchor !== anchor || keyboard) void open(metric, anchor, false, keyboard);
  }

  function showForecast(metric: QuotaMetric, anchor: HTMLButtonElement)
  {
    cancelTimers();
    pinned = false;
    if (active?.anchor !== anchor) void open(metric, anchor, true);
  }

  async function resetForecast(key: QuotaKey)
  {
    const id = JSON.stringify(key);
    if (resetState[id] === 'pending') return;
    dismiss();
    resetState[id] = 'pending';
    try
    {
      onUpdate(await backend.resetQuotaForecast(key));
      delete resetState[id];
    }
    catch { resetState[id] = 'failed'; }
  }

  function dismiss()
  {
    cancelTimers();
    if (forecastTip?.matches(':popover-open')) forecastTip.hidePopover();
    active = null;
    historyTop = null;
    pinned = false;
  }

  function outside(event: PointerEvent)
  {
    const target = event.target;
    if (!active || !(target instanceof Node) || active.anchor.contains(target) || forecastTip?.contains(target)) return;
    // A title-bar drag should move a pinned card along with the window.
    if (pinned && target instanceof Element && target.hasAttribute('data-tauri-drag-region')) return;
    dismiss();
  }

  function escape(event: KeyboardEvent)
  {
    if (event.key !== 'Escape' || !active) return;
    event.preventDefault();
    closeAndRestoreFocus();
  }

  function closeAndRestoreFocus()
  {
    const anchor = active?.forecast ? undefined : active?.anchor;
    // Restoring focus can expose another quota row under a stationary pointer.
    hoverBlocked = true;
    dismiss();
    anchor?.focus({ preventScroll: true });
    cancelTimers();
  }
</script>

<svelte:window onresize={position} onpointerdown={outside} onkeydown={escape} />

{#snippet windowRow(provider: ToolQuota, window: QuotaWindow)}
  {@const metric = windowMetric(provider, window)}
  {@const resetting = resetState[JSON.stringify(metric.key)]}
  <div class="limits-window">
    <button class="limits-reading" type="button" aria-label={metric.label + ' · ' + metric.current}
      aria-haspopup="dialog" aria-expanded={activeKey === JSON.stringify(metric.key)}
      onpointerenter={event => pointerReading(event, metric)} onpointermove={event => pointerReading(event, metric)}
      onpointerleave={leave} onfocus={event => hover(metric, event.currentTarget, 0)} onblur={leave}
      onclick={event => select(metric, event)}>
      <span class="limits-reading-line">
        <span>{duration(window) || local('Quota', '额度')}</span><b>{metric.current}</b>
      </span>
      <span class="limits-meter" aria-hidden="true"><span style:--remaining={window.remaining + '%'}></span></span>
      <span class="quota-reset" title={time(window.resetsAt)}>{reset(window.resetsAt, now)}</span>
    </button>
    {#if window.minutes === 300 || window.minutes === 10080}
      <div class="limits-forecast">
        <span aria-live="polite">{forecastLabel(window, tracked[provider.tool], now)}</span>
        <button class="quota-banked-info forecast-info" type="button"
          aria-label={metric.label + ' · ' + local('Usage this cycle', '本周期已用量')}
          aria-describedby={active?.forecast && active.metric.label === metric.label && active.metric.key.tool === provider.tool ? 'quota-forecast-tooltip' : undefined}
          onpointerenter={event => pointerReading(event, metric, true)} onpointermove={event => pointerReading(event, metric, true)} onpointerleave={leave}
          onfocus={event => showForecast(metric, event.currentTarget)} onblur={leave}
          onclick={event => showForecast(metric, event.currentTarget)}>!</button>
        {#if tracked[provider.tool]}
          <button type="button" class="forecast-reset" disabled={resetting === 'pending'}
            aria-label={[local('Reset forecast', '重置预测'), toolInfo(provider.tool).name, metric.label].join(' · ')}
            title={local('Restart this forecast using new samples', '使用新样本重新预测此窗口')}
            onclick={() => resetForecast(metric.key)}>
            {resetting === 'pending' ? local('Resetting…', '重置中…') : local('Reset', '重置')}
          </button>
        {/if}
      </div>
      {#if resetting === 'failed'}<p class="forecast-reset-error" role="alert">{local('Reset failed. Try again.', '重置失败，请重试。')}</p>{/if}
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
          aria-haspopup="dialog" aria-expanded={activeKey === JSON.stringify(metric.key)}
          onpointerenter={event => pointerReading(event, metric)} onpointermove={event => pointerReading(event, metric)}
          onpointerleave={leave} onfocus={event => hover(metric, event.currentTarget, 0)} onblur={leave}
          onclick={event => select(metric, event)}>
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

<div id="quota-forecast-tooltip" class="forecast-tip" popover="manual" bind:this={forecastTip}
  role="tooltip" aria-label={local('Usage this cycle', '本周期已用量')}
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
          forecast.basis === 'current' ? local('Current cycle', '本期推算') : forecastLabel(activeWindow, tracked[active.metric.key.tool], now)}</small>
      {#if forecast.totalTokens != null}
        <small>{forecast.modelMixRequests
          ? local(`Model mix · last ${forecast.modelMixRequests} calls`, `按最近 ${forecast.modelMixRequests} 次调用的模型组合`)
          : local('Window average', '按窗口平均用量')}</small>
      {/if}
      {#if forecast.partial}<small>{local('Partial local records', '本机记录不完整')}</small>{/if}
      <small>{local('As of ', '截至 ')}{time(forecast.observedAt)}</small>
    {:else}
      {forecastLabel(activeWindow, tracked[active.metric.key.tool], now)}
    {/if}
    {#if activeWindow.forecastResetAt != null}
      <small>{local('Forecast reset ', '预测重置于 ')}{time(activeWindow.forecastResetAt)}</small>
    {/if}
  {/if}
</div>
