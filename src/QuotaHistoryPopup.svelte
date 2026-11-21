<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { backend } from './api';
  import type { QuotaHistory } from './api';
  import { local, toolInfo } from './display';
  import { setLanguage } from './language.svelte';
  import { quotaPopup } from './quota-popup';
  import type { PopupSelection } from './quota-popup';
  import QuotaHistoryCard from './QuotaHistoryCard.svelte';

  let selection = $state<PopupSelection | null>(null);
  let history = $state<QuotaHistory | null>(null);
  let failed = $state(false);
  let frame: HTMLDivElement | undefined = $state();
  let contents: HTMLDivElement | undefined = $state();
  const remembered = new Map<string, { revision: number | null; history: QuotaHistory }>();
  let selectedKey = $derived(selection ? JSON.stringify(selection.key) : '');
  let revision = $derived(selection?.revision ?? null);

  onMount(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    quotaPopup.onSelection(next => { selection = next; }).then(unlisten => {
      if (disposed) unlisten();
      else
      {
        unsubscribe = unlisten;
        void quotaPopup.report({ kind: 'ready' }).catch(() => {});
      }
    }).catch(() => {});
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  });

  $effect(() => {
    if (!selection) return;
    setLanguage(selection.language);
    document.documentElement.style.setProperty('--ui-text-scale', String(selection.fontScale / 100));
  });

  $effect(() => {
    const key = selectedKey;
    const version = revision;
    if (!key) return;
    const previous = remembered.get(key);
    history = previous?.history ?? null;
    failed = false;
    if (previous?.revision === version) return;
    let cancelled = false;
    backend.quotaHistory(JSON.parse(key)).then(result => {
      if (cancelled) return;
      remembered.set(key, { revision: version, history: result });
      history = result;
    }).catch(() => {
      if (!cancelled && !previous) failed = true;
    });
    return () => { cancelled = true; };
  });

  $effect(() => {
    const id = selection?.id;
    const element = contents;
    if (id == null || !element) return;
    let lastHeight = 0;
    const measure = () => {
      if (selection?.id !== id) return;
      const height = Math.ceil(element.getBoundingClientRect().height);
      if (!height || height === lastHeight) return;
      lastHeight = height;
      void quotaPopup.report({ kind: 'size', id, height }).catch(() => {});
    };
    // Measure once even if WebView2 throttles resize notifications while the host is hidden.
    void tick().then(measure);
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  });

  function interact(kind: 'enter' | 'leave' | 'close')
  {
    if (selection) void quotaPopup.report({ kind, id: selection.id }).catch(() => {});
  }

  function escape(event: KeyboardEvent)
  {
    if (event.key !== 'Escape' || !selection) return;
    event.preventDefault();
    interact('close');
  }
</script>

<svelte:window onkeydown={escape}
  onfocus={() => { if (document.activeElement === document.body) frame?.focus({ preventScroll: true }); }} />

{#if selection}
  <div class="quota-history-popup" bind:this={frame} tabindex="-1" role="dialog" aria-label={local('Quota history', '额度使用记录')}
    style:--row-ink={toolInfo(selection.key.tool).color} onpointerenter={() => interact('enter')} onpointerleave={() => interact('leave')}>
    <div class="quota-history-content" bind:this={contents}>
      {#key selectedKey}
        <QuotaHistoryCard name={toolInfo(selection.key.tool).name} key={selection.key} label={selection.label}
          current={selection.current} resetAt={selection.resetAt} {history} {failed} />
      {/key}
    </div>
  </div>
{/if}
