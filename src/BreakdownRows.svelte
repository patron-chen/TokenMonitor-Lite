<script lang="ts">
  import type { Breakdown, ToolMode } from './api';
  import { cost, modelIcon, number, toolInfo, usageSplits } from './display';

  interface Props
  {
    rows: Breakdown[];
    kind: 'tool' | 'model';
    mode?: ToolMode;
    expanded?: string | null;
    activate: (key: string) => void;
    toolRows?: Breakdown[];
  }

  let { rows, kind, mode = 'tokens', expanded = null, activate, toolRows = [] }: Props = $props();
  let maximum = $derived(Math.max(1, ...rows.map(row => row.usage.tokens)));
</script>

{#each rows as row (row.key)}
  {@const toolKey = kind === 'tool' ? row.key : toolRows.find(tool => tool.models.some(model => model.key === row.key))?.key ?? ''}
  {@const tool = toolInfo(toolKey)}
  {@const icon = kind === 'tool' ? tool.icon : modelIcon(row.key, toolKey)}
  <article class="usage-row breakdown-row" style:--row-ink={tool.color}>
    <button class="usage-trigger" onclick={() => activate(row.key)} aria-expanded={expanded === row.key}>
      <div class="usage-heading">
        <span class="row-name">
          {#if icon}<span class="icon" style:--icon={'url(' + icon + ')'} aria-hidden="true"></span>{/if}
          <span class="ellipsis">{kind === 'tool' ? tool.name : row.key}</span>
        </span>
        <span class="row-value">
          <span>{number(row.usage.tokens)}</span>
          <span class="row-cost">{cost(row.usage)}</span>
        </span>
      </div>
      <span class="bar" aria-hidden="true"><span style:--share={row.usage.tokens > 0 ? Math.max(.02, row.usage.tokens / maximum) : 0}></span></span>
    </button>
    {#if expanded === row.key}
      <dl class="splits">
        {#if mode === 'models' && kind === 'tool' && row.models.length}
          {#each row.models as model (model.key)}
            <div><dt>{model.key}</dt><dd>{number(model.usage.tokens)}</dd></div>
          {/each}
        {:else}
          {#each usageSplits(row.usage.counts) as item (item.key)}
            <div>
              <dt>
                <span>{item.label}</span>
                {#if item.share !== null}<span class="split-share">{item.share}</span>{/if}
              </dt>
              <dd>{item.value}</dd>
            </div>
          {/each}
        {/if}
      </dl>
    {/if}
  </article>
{/each}
