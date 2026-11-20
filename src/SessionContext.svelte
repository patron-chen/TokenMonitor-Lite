<script lang="ts">
  import type { Session } from './api';
  import { compact, local, sessionState } from './display';

  let { session, now }: { session: Session; now: number } = $props();
  let context = $derived(session.context && sessionState(session, now) !== 'idle' &&
    now - session.context.at <= 10 * 60_000 ? session.context : null);
</script>

{#if context}
  {@const left = Math.max(0, Math.round((context.window - context.tokens) / context.window * 100))}
  {@const used = 100 - left}
  <span class="session-context" data-tone={left <= 10 ? 'low' : left <= 30 ? 'caution' : ''}
    role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={used}
    aria-label={local('Context used', '上下文已用')}
    title={local('Context used ', '上下文已用 ') + compact(context.tokens) + ' / ' + compact(context.window)}>
    <span class="session-context-meter" aria-hidden="true"><span style:--share={used / 100}></span></span>
    <span>{used}%</span>
  </span>
{/if}
