<script lang="ts">
  import type { Quotas, ToolId } from './api';
  import { icons, local, toolInfo } from './display';
  import QuotaRows from './QuotaRows.svelte';

  interface Props
  {
    data: Quotas | null;
    loading: boolean;
    now: number;
    openLimits: (tool?: ToolId) => void;
  }

  let { data, loading, now, openLimits }: Props = $props();
</script>

<section class="home-section quota-section quota-jump"
  aria-label={local('Quotas', '额度')} aria-busy={loading}>
  <header class="section-heading">
    <h2>
      <button class="section-title-action" type="button" onclick={() => openLimits()}
        title={local('Open Limits', '打开额度页')}>
        {local('Quotas', '额度')} <span class="chevron" aria-hidden="true"></span>
      </button>
    </h2>
    <span class="section-action" aria-hidden="true">
      <span class="icon" style:--icon={'url(' + icons.limits + ')'}></span>
    </span>
  </header>
  {#if data?.tools.length}
    {#each data.tools as provider (provider.tool)}
      {@const tool = toolInfo(provider.tool)}
      <article class="quota-tool">
        <button class="quota-provider quota-provider-link" type="button" onclick={() => openLimits(provider.tool)}
          aria-label={local('Open Limits for ' + tool.name, '打开 ' + tool.name + ' 额度')}>
          <span class="row-name">
            <span class="icon" style:--icon={'url(' + tool.icon + ')'} aria-hidden="true"></span>
            <span class="ellipsis">{tool.name}</span>
          </span>
          {#if provider.plan}<span class="row-meta ellipsis">{provider.plan}</span>{/if}
        </button>
        <QuotaRows {provider} {now} openTool={() => openLimits(provider.tool)} />
      </article>
    {/each}
  {:else}
    <p class="notice" role="status">{loading ? local('Reading tool quotas…', '正在读取工具额度…') :
      local('No tool quota available yet.', '暂无可读的工具额度。')}</p>
  {/if}
</section>
