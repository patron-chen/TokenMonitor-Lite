<script lang="ts">
  import type { QuotaWindow, ToolQuota } from './api';
  import { local, time, toolInfo } from './display';
  import { isChinese } from './language.svelte';
  import { duration, quotaGroups, quotaMoney, remainingPercent, reset } from './quota-display';
  import QuotaBanked from './QuotaBanked.svelte';

  interface Props
  {
    provider: ToolQuota;
    now: number;
    openTool?: () => void;
  }

  let { provider, now, openTool }: Props = $props();
  let byModel = $derived(provider.windows.some(window => window.group.trim()));
  let tool = $derived(toolInfo(provider.tool));
  let zh = $derived(isChinese());
</script>

{#snippet quotaMetric(window: QuotaWindow, group: string)}
  {@const range = duration(window)}
  {@const remaining = remainingPercent(window.remaining)}
  {@const resetAt = reset(window.resetsAt, now)}
  <div class="quota-window">
    <div class="quota-line">
      <span class="ellipsis" title={group || undefined}>{group || range || local('Quota', '额度')}</span>
      <b class="quota-value">{local(remaining + ' left', '剩余 ' + remaining)}</b>
    </div>
    {#if resetAt || (group && range)}
      <span class="quota-reset ellipsis" title={time(window.resetsAt)}>{[group && range, resetAt].filter(Boolean).join(' · ')}</span>
    {/if}
  </div>
{/snippet}

{#snippet compactQuota(window: QuotaWindow)}
  {@const range = duration(window)}
  {@const remaining = remainingPercent(window.remaining)}
  {@const resetAt = reset(window.resetsAt, now, true)}
  {@const awaiting = window.resetsAt != null && window.resetsAt <= now}
  <div class="quota-window quota-compact-line" class:no-period={!range}>
    {#if range}
      <span class="quota-period">{range}</span>
      <span class="quota-dot" aria-hidden="true">·</span>
    {/if}
    <b class="quota-value" title={local(remaining + ' left', '剩余 ' + remaining)}>
      {#if zh}<span class="quota-prefix">{'剩余 '}</span>{/if}{remaining}
      {#if !zh}<span class="quota-prefix">{' left'}</span>{/if}
    </b>
    {#if resetAt}
      <span class="quota-dot" aria-hidden="true">·</span>
      {#if awaiting}
        <span class="quota-reset ellipsis" title={resetAt}>{resetAt}</span>
      {:else}
        <span class="quota-reset quota-compact-reset" title={reset(window.resetsAt, now)}>
          {#if !zh}<span class="quota-reset-word ellipsis">Reset</span>{/if}
          <span class="quota-countdown">{resetAt}</span>
          {#if zh}<span class="quota-reset-word ellipsis">后重置</span>{/if}
        </span>
      {/if}
    {/if}
  </div>
{/snippet}

<div class="quota-grid" class:quota-model-grid={byModel}>
  {#if byModel}
    {#each Array.from(quotaGroups(provider.windows)) as [label, windows] (label)}
      <div class="quota-model" role="group" aria-label={label || tool.name}>
        {#if label}<span class="quota-model-label ellipsis" title={label}>{label}</span>{/if}
        {#each windows as window}
          {@render compactQuota(window)}
        {/each}
      </div>
    {/each}
  {:else}
    {#each provider.windows as window}
      {@render quotaMetric(window, window.group)}
    {/each}
  {/if}
  {#each provider.balances as balance (balance.currency)}
    <div class="quota-window">
      <div class="quota-line">
        <span>{local('API balance', 'API 余额')}</span>
        <b class="quota-value">{quotaMoney(balance.amount, balance.currency)}</b>
      </div>
    </div>
  {/each}
</div>
<QuotaBanked bank={provider.bankedReset} {now} {openTool} />
