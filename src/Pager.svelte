<script lang="ts">
  import { pageSize } from './api';
  import { local, number } from './display';
  let { page, total, change }: { page: number; total: number; change: (page: number) => void } = $props();
</script>

{#if total > pageSize}
  <div class="pager" role="group" aria-label={local('Pagination', '分页')}>
    <button class="arrow prev" disabled={page === 0} onclick={() => change(page - 1)} aria-label={local('Previous page', '上一页')}>←</button>
    <span>{number(page * pageSize + 1)}–{number(Math.min(total, (page + 1) * pageSize))} / {number(total)}</span>
    <button class="arrow next" disabled={(page + 1) * pageSize >= total} onclick={() => change(page + 1)} aria-label={local('Next page', '下一页')}>→</button>
  </div>
{/if}
