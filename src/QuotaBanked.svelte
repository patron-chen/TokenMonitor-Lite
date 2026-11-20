<script lang="ts">
  import type { ToolQuota } from './api';
  import { local, time } from './display';
  import { formatCountdown } from './quota-display';

  let { bank, now, openTool }: { bank: ToolQuota['bankedReset']; now: number; openTool?: () => void } = $props();
  const expiryId = $props.id();
  let expiryTip: HTMLDivElement | undefined = $state();

  function expiryCountdown(at: number, now: number)
  {
    if (at <= now) return 'now';
    const minutes = Math.round((at - now) / 60000);
    return minutes ? formatCountdown(minutes) : '<1m';
  }

  function showExpiry(show: boolean)
  {
    if (!expiryTip || expiryTip.matches(':popover-open') === show) return;
    if (show) expiryTip.showPopover();
    else expiryTip.hidePopover();
  }
</script>

{#if bank?.availableCount}
  <div class="quota-banked" role="group" aria-label="Banked Reset">
    <span>{bank.availableCount} {bank.availableCount === 1 ? 'reset' : 'resets'}</span>
    {#if bank.expirations.length}
      <span class="quota-banked-expiries">
        {#each bank.expirations.slice(0, 3) as at, index}
          <span>
            {#if index > 0}<span aria-hidden="true">·</span>{/if}
            {expiryCountdown(at, now)}
          </span>
        {/each}
        {#if bank.expirations.length > 3}<span>· +{bank.expirations.length - 3}</span>{/if}
      </span>
      <button class="quota-banked-info" type="button" aria-label={local('Banked Reset expirations', 'Banked Reset 到期明细')}
        aria-describedby={expiryId} onpointerenter={() => showExpiry(true)} onpointerleave={() => showExpiry(false)}
        onfocus={() => showExpiry(true)} onblur={() => showExpiry(false)}
        onclick={() => openTool ? openTool() : showExpiry(true)}
        onkeydown={event => {
          if (event.key === 'Escape') { event.stopPropagation(); showExpiry(false); }
        }}>i</button>
      <div class="quota-banked-tooltip" id={expiryId} popover="manual" role="tooltip" bind:this={expiryTip}>
        {#each bank.expirations as at}
          <time datetime={new Date(at).toISOString()}>{time(at)}</time>
          <span>{expiryCountdown(at, now)}</span>
        {/each}
      </div>
    {/if}
  </div>
{/if}
