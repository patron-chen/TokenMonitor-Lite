<script lang="ts">
  import type { Session } from './api';
  import { effortTitle, sessionModelLabel, sessionModelsTitle } from './display';

  let { session }: { session: Session } = $props();
  let model = $derived(sessionModelLabel(session));
</script>

{#if model}<span class="session-model-label" title={sessionModelsTitle(session)}>{model}</span>{/if}
{#if session.reasoningEffort}
  {#if model}<span class="session-meta-dot" aria-hidden="true">{' · '}</span>{/if}<span class="session-effort" title={effortTitle(session.reasoningEffort)}>{session.reasoningEffort}</span>
{/if}
