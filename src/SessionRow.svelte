<script lang="ts">
  import type { Session } from './api';
  import { compact, cost, icons, local, number, relativeTime, sessionActivityParts, sessionKey,
    sessionModelLabel, sessionState, sessionTitle, shortSessionTime, time, toolInfo } from './display';
  import SessionContext from './SessionContext.svelte';
  import SessionModel from './SessionModel.svelte';

  interface Props { row: Session; maximum: number; open: (row: Session) => void; now: number; preview?: boolean }
  let { row, maximum, open, now, preview = false }: Props = $props();
  let tool = $derived(toolInfo(row.tool));
  let state = $derived(sessionState(row, now));
  let activity = $derived(sessionActivityParts(row, now, preview ? 'preview' : 'list'));
  let sessionTime = $derived(row.lastUsedAt ?? row.metrics.startedAt);
  let age = $derived(preview ? relativeTime(sessionTime, now) : '');
  let model = $derived(sessionModelLabel(row) || row.reasoningEffort);
  let agents = $derived(row.metrics.subagents > 0 ? local(
    `${row.metrics.subagents} ${row.metrics.subagents === 1 ? 'subagent' : 'subagents'}`,
    `${row.metrics.subagents} 个子代理`
  ) : '');
  const stateLabel = {
    running: local('Running', '进行中'), waiting: local('Waiting for input', '等待输入'),
    ended: local('Finished', '已完成'), idle: ''
  };
</script>

{#snippet separator()}
  <span class="session-meta-dot" aria-hidden="true">{' · '}</span>
{/snippet}

{#snippet statistics()}
  {#each activity as part, index (part.label)}
    {#if index > 0}{@render separator()}{/if}<span class="session-stat" title={part.label || undefined}>{part.value}</span>
  {/each}
{/snippet}

<article class="usage-row" class:preview style:--row-ink={tool.color}>
  <button class="usage-trigger session-trigger" type="button" onclick={() => open(row)} data-session-key={sessionKey(row)}>
    <div class="session-main">
      <div class="row-name">
        <span class="icon" style:--icon={'url(' + tool.icon + ')'} title={tool.name} aria-hidden="true"></span>
        {#if preview || state !== 'idle'}
          <span class="session-state" data-state={state} role="img" aria-label={stateLabel[state] || undefined}
            aria-hidden={state === 'idle'} title={stateLabel[state]}>
            {#if state === 'running'}
              <span class="icon" style:--icon={'url(' + icons.spinner + ')'} aria-hidden="true"></span>
            {:else if state === 'ended' || state === 'waiting'}
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                {#if state === 'ended'}<path d="M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18 M8.7 12.2l2.1 2.1 4.5-4.6" />
                {:else}<path d="M8 5v14 M16 5v14" />{/if}
              </svg>
            {/if}
          </span>
        {/if}
        <span class="ellipsis" title={sessionTitle(row)}>{sessionTitle(row)}</span>
      </div>
      <div class="row-value">
        <b title={number(row.usage.tokens)}>{compact(row.usage.tokens)}</b>
        {#if !preview}<span class="row-cost">{cost(row.usage)}</span>{/if}
      </div>
      <div class="session-subline">
        <span class="row-meta ellipsis">
          {#if !preview}{tool.name}{#if model || agents}{' · '}{/if}{/if}<SessionModel session={row} />
          {#if agents}{#if model}{@render separator()}{/if}<span class:ellipsis={preview}>{agents}</span>{/if}
          {#if age}
            {#if model || agents}{@render separator()}{/if}<span class="session-age ellipsis" title={time(sessionTime)}>
              <span class="session-age-relative">{age}</span><span class="session-age-short">{shortSessionTime(sessionTime, now)}</span>
            </span>
          {/if}
          {#if preview && activity.length}
            {#if model || agents || age}{@render separator()}{/if}{@render statistics()}
          {/if}
        </span>
        {#if preview}<SessionContext session={row} {now} />{/if}
      </div>
      {#if !preview}
        <div class="session-activity">
          <span class="row-meta ellipsis">{@render statistics()}</span>
          <SessionContext session={row} {now} />
        </div>
      {/if}
    </div>
    {#if !preview}<span class="bar" aria-hidden="true"><span style:--share={row.usage.tokens > 0 ? Math.max(.02, row.usage.tokens / maximum) : 0}></span></span>{/if}
  </button>
</article>
