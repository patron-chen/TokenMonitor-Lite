<script lang="ts">
  import { toolIds } from './api';
  import type { Snippet } from 'svelte';
  import type { Appearance, Backdrop, Cadence, CollectionStatus, Language, Preferences, SettingsSection, StatisticsRange, ToolId } from './api';
  import { compact, icons, local, number, statisticsRangeLabels, time, toolInfo } from './display';
  import AboutPanel from './AboutPanel.svelte';
  import ProxyPanel from './ProxyPanel.svelte';

  interface Props
  {
    appearance: Appearance;
    backdrop: Backdrop;
    preferences: Preferences;
    section: SettingsSection | null;
    collection: CollectionStatus | null;
    busy: { settings: boolean; collection: boolean };
    errors: { settings: boolean; appearance: boolean; collection: boolean };
    selectSection: (section: SettingsSection | null) => void;
    saveAppearance: () => void;
    setPreferences: (preferences: Preferences) => void;
    refreshCollection: () => void;
  }

  let { appearance = $bindable(), backdrop, preferences, section, collection, busy, errors,
    selectSection, saveAppearance, setPreferences, refreshCollection }: Props = $props();

  const sections: { id: SettingsSection; name: string; icon: string }[] = $derived([
    { id: 'appearance', name: local('Appearance', '外观'), icon: icons.appearance },
    { id: 'collection', name: local('Collection', '采集'), icon: icons.collection },
    { id: 'quotas', name: local('AI Tool Limits', 'AI 工具额度'), icon: icons.limits },
    { id: 'network', name: local('Network proxy', '网络代理'), icon: icons.network },
    { id: 'about', name: local('About Lite', '关于 Lite'), icon: icons.about }
  ]);
  const sizes: Record<number, string> = $derived({
    100: local('Standard', '标准'), 110: local('Larger', '较大'), 120: local('Largest', '最大')
  });
  const cadences: Record<Cadence, string> = $derived({
    '5m': local('5 minutes', '5 分钟'),
    '15m': local('15 minutes', '15 分钟'),
    live: local('20 minutes (default)', '20 分钟（默认）'),
    '30m': local('30 minutes', '30 分钟')
  });
  const quotaSetup: Record<ToolId, { methods: string[]; instructions: string }> = $derived({
    codex: {
      methods: [local('Local logs', '本地日志'), 'OAuth'],
      instructions: local('Sign in to Codex to fetch live quota windows at startup, on manual refresh and at every idle tray check. Panel refreshes also read Banked Reset.',
        '登录 Codex 后，Lite 会在启动、手动刷新和每次托盘空闲检查时联网读取周期额度。面板刷新时也会查询 Banked Reset。')
    },
    antigravity: {
      methods: ['RPC'],
      instructions: local('Keep Antigravity CLI or IDE running locally to make its quotas available.',
        '保持本机 Antigravity CLI 或 IDE 运行，即可读取额度。')
    },
    dsh: {
      methods: [local('API key', 'API 密钥')],
      instructions: local('Set DEEPSEEK_API_KEY or DEEPSEEK_KEY and restart the tray app to enable API balance.',
        '设置 DEEPSEEK_API_KEY 或 DEEPSEEK_KEY 后，从托盘退出并重新启动 Lite，即可读取 API 余额。')
    }
  });
  let expandedTool = $state<ToolId | null>(null);
  let tracked = $derived(toolIds.filter(tool => preferences.collection.tools[tool]).length);
  let enabledQuotas = $derived(toolIds.filter(tool => preferences.quotas.tools[tool]).length);
  let collecting = $derived(!!collection?.scanning);
  let quotaMinutes = $derived(preferences.quotas.refreshMinutes);

  function summary(id: SettingsSection)
  {
    if (id === 'appearance') return sizes[appearance.fontScale] + ' · ' + appearance.opacity + '%';
    if (id === 'collection') return local(`${tracked} tools · live`, `${tracked} 个工具 · 实时`);
    if (id === 'quotas') return local(`${enabledQuotas} enabled · ${quotaMinutes} min`, `启用 ${enabledQuotas} · ${quotaMinutes} 分钟`);
    return '';
  }

  function switchTool(area: 'collection' | 'quotas', tool: ToolId, enabled: boolean)
  {
    setPreferences({ ...preferences, [area]: {
      ...preferences[area], tools: { ...preferences[area].tools, [tool]: enabled }
    } });
  }

  function scanStatus()
  {
    if (collecting) return local('Collecting…', '正在采集…');
    if (!tracked) return local('No tools enabled', '未启用采集工具');
    if (collection?.failure) return local('Last scan failed', '上次采集失败');
    if (!collection) return '—';
    return collection.watching ? local('Watching', '正在监看') : local('Watch unavailable', '监看不可用');
  }
</script>

{#snippet toolRow(area: 'collection' | 'quotas', tool: ToolId, details: Snippet)}
  {@const info = toolInfo(tool)}
  {@const enabled = preferences[area].tools[tool]}
  {@const expanded = expandedTool === tool}
  {@const detailsId = 'settings-' + area + '-' + tool}
  <div class="settings-tool">
    <div class="settings-tool-row">
      <input class="settings-check" type="checkbox" checked={enabled} disabled={busy.settings}
        aria-label={area === 'collection' ? local('Collect ' + info.name, '采集 ' + info.name)
          : local('Read ' + info.name + ' limits', '读取 ' + info.name + ' 额度')}
        onchange={event => switchTool(area, tool, event.currentTarget.checked)} />
      <button class="settings-tool-toggle" type="button" aria-expanded={expanded} aria-controls={detailsId}
        onclick={() => { expandedTool = expanded ? null : tool; }}>
        <span class="settings-tool-name">
          <span class="icon" style:--icon={'url(' + info.icon + ')'} aria-hidden="true"></span>
          <span class="ellipsis">{info.name}</span>
          {#if area === 'collection' && enabled}
            <span class="settings-tool-tag tracking">{local('Tracked', '追踪中')}</span>
          {/if}
        </span>
        <span class="chevron" aria-hidden="true"></span>
        {#if area === 'quotas'}
          <span class="settings-tool-methods">
            {#each quotaSetup[tool].methods as method (method)}
              <span class="settings-tool-tag">{method}</span>
            {/each}
          </span>
        {/if}
      </button>
    </div>
    <div class="settings-tool-details" id={detailsId} hidden={!expanded}>
      {#if expanded}{@render details()}{/if}
    </div>
  </div>
{/snippet}

<section id="settings-panel" class="viewport settings-panel" aria-label={local('Settings', '设置')}>
  <div class="settings-stack">
    {#each sections as tab (tab.id)}
      {@const open = section === tab.id}
      {@const aside = summary(tab.id)}
      <section class="settings-section" class:expanded={open}>
        <button class="settings-section-toggle" type="button" id={'settings-' + tab.id + '-label'}
          aria-expanded={open} aria-controls={'settings-' + tab.id}
          onclick={() => {
            expandedTool = null;
            selectSection(open ? null : tab.id);
          }}>
          <span class="settings-section-name">
            <span class="icon" style:--icon={'url(' + tab.icon + ')'} aria-hidden="true"></span>
            {tab.name}
          </span>
          <span class="settings-section-aside">
            {#if aside}<span class="ellipsis">{aside}</span>{/if}
            <span class="chevron" aria-hidden="true"></span>
          </span>
        </button>
        <div id={'settings-' + tab.id} class="settings-body" role="region"
          aria-labelledby={'settings-' + tab.id + '-label'} hidden={!open}>
          {#if open && tab.id === 'appearance'}
            <div class="settings-item">
              <label for="language">{local('Language', '语言')}</label>
              <select id="language" value={preferences.language} disabled={busy.settings}
                onchange={event => setPreferences({ ...preferences, language: event.currentTarget.value as Language })}>
                <option value="system">{local('Follow system', '跟随系统')}</option>
                <option value="zh-CN" lang="zh-CN">简体中文</option>
                <option value="en" lang="en">English</option>
              </select>
            </div>
            <div class="settings-item">
              <label for="background-opacity">{local('Background opacity', '背景不透明度')}</label>
              <b>{appearance.opacity}%</b>
              <input class="settings-range" id="background-opacity" type="range" min="65" max="100"
                bind:value={appearance.opacity} onchange={saveAppearance} />
            </div>
            <div class="settings-item">
              <label for="text-scale">{local('Text size', '字体大小')}</label>
              <select id="text-scale" value={appearance.fontScale} onchange={event => {
                appearance.fontScale = Number(event.currentTarget.value);
                saveAppearance();
              }}>
                {#each [100, 110, 120] as size (size)}<option value={size}>{sizes[size]}</option>{/each}
              </select>
            </div>
            <div class="settings-item settings-fact">
              <span>{local('Window backdrop', '窗口材质')}</span>
              <span>{backdrop === 'none' ? local('Translucent', '半透明') : local('Frosted glass', '毛玻璃')}</span>
            </div>
            {#if errors.appearance}<p class="settings-note error" role="status">{local('Could not save appearance.', '无法保存外观设置。')}</p>{/if}
          {:else if open && tab.id === 'collection'}
            <div class="settings-item">
              <label for="statistics-range">{local('Statistics range', '统计范围')}</label>
              <select id="statistics-range" value={preferences.statisticsRange} disabled={busy.settings}
                aria-describedby="statistics-range-note" onchange={event => setPreferences({
                  ...preferences, statisticsRange: event.currentTarget.value as StatisticsRange
                })}>
                {#each Object.entries(statisticsRangeLabels()) as [value, label] (value)}<option {value}>{label}</option>{/each}
              </select>
              <p id="statistics-range-note" class="settings-note">
                {local('Token totals on the middle period tab.', '用于右上角中间页签的 Token 汇总。')}
              </p>
            </div>
            <div class="settings-item">
              <label for="collection-cadence">{local('Maximum idle tray interval', '托盘空闲最长间隔')}</label>
              <select id="collection-cadence" value={preferences.collection.cadence} disabled={busy.settings}
                aria-describedby="collection-cadence-note" onchange={event => setPreferences({
                  ...preferences, collection: { ...preferences.collection, cadence: event.currentTarget.value as Cadence }
                })}>
                {#each Object.entries(cadences) as [value, label] (value)}<option {value}>{label}</option>{/each}
              </select>
              <p id="collection-cadence-note" class="settings-note">
                {local('Opening the window or hovering over the tray icon refreshes usage immediately. Idle checks start at 1 minute and gradually reach this limit. Each check refreshes enabled quotas and collects pending log changes.',
                  '打开窗口或悬停托盘图标会立即刷新用量。收起窗口且鼠标离开后，检查间隔从 1 分钟起逐步延长到此上限。每次都查询已启用的额度，并合并采集期间的日志变化。')}
              </p>
            </div>
            <div class="settings-item">
              <span>{local('Today’s collection', '今日采集')}</span>
              <button class="settings-action" type="button" onclick={refreshCollection} disabled={collecting || busy.collection || !tracked}>
                {local('Collect now', '立即采集')}
              </button>
              <details class="settings-collection-status">
                <summary>
                  <span>{local('Collection status', '采集状态')}</span>
                  <span aria-live="polite">{scanStatus()}</span>
                </summary>
                <dl class="settings-facts">
                  <div><dt>{local('Last successful scan', '最近成功采集')}</dt><dd>{time(collection?.updatedAt ?? null) || '—'}</dd></div>
                  <div><dt>{local('Time zone', '统计时区')}</dt><dd>{collection?.timezone || '—'}</dd></div>
                </dl>
              </details>
              {#if errors.collection}<p class="settings-note error" role="status">{local('Could not read collection status.', '无法读取采集状态。')}</p>{/if}
            </div>
            <div class="settings-tool-list">
              {#each toolIds as tool (tool)}
                {#snippet collectionDetails()}
                  {@const data = collection?.sources.find(source => source.tool === tool)}
                  {#if data}
                    <dl class="settings-source">
                      <div>
                        <dt>{local('Log directories', '日志目录')}</dt>
                        <dd>
                          <div>{local('Found', '已找到')} {data.roots.filter(root => root.exists).length} / {data.roots.length}</div>
                          <ul>
                            {#each data.roots as root (root.path)}
                              <li><code>{root.path}</code><span>{root.exists ? local('Found', '已找到') : local('Missing', '未找到')}</span></li>
                            {/each}
                          </ul>
                        </dd>
                      </div>
                      <div>
                        <dt>{local('Today’s usage', '今日用量')}</dt>
                        {#if preferences.collection.tools[tool] && data.tokens != null}
                          <dd><b title={number(data.tokens)}>{compact(data.tokens)} Token</b></dd>
                        {:else}
                          <dd>—</dd>
                        {/if}
                      </div>
                    </dl>
                  {:else}
                    <p class="settings-note">{busy.collection ? local('Reading collection status…', '正在读取采集状态…')
                      : local('No source data available.', '暂无采集来源数据。')}</p>
                  {/if}
                {/snippet}
                {@render toolRow('collection', tool, collectionDetails)}
              {/each}
            </div>
          {:else if open && tab.id === 'quotas'}
            <div class="settings-tool-list">
              {#each toolIds as tool (tool)}
                {#snippet setupDetails()}
                  <p class="settings-note">{quotaSetup[tool].instructions}</p>
                {/snippet}
                {@render toolRow('quotas', tool, setupDetails)}
              {/each}
            </div>
            <div class="settings-item">
              <label for="quota-frequency">{local('Refresh frequency', '刷新频率')}</label>
              <select id="quota-frequency" value={quotaMinutes} disabled={busy.settings} onchange={event => setPreferences({
                ...preferences, quotas: { ...preferences.quotas, refreshMinutes: Number(event.currentTarget.value) }
              })}>
                {#each [1, 2, 5, 15, 30] as minutes (minutes)}
                  <option value={minutes}>{local(`${minutes} min`, `${minutes} 分钟`)}</option>
                {/each}
              </select>
              <p class="settings-note">{local('Applies to the Home and Limits views. In the tray, quotas follow the idle check interval.', '用于主页和额度页的刷新。托盘空闲时，额度按托盘检查间隔查询。')}</p>
            </div>
          {:else if open && tab.id === 'network'}
            <ProxyPanel />
          {:else if open && tab.id === 'about'}
            <AboutPanel autoUpdate={preferences.autoUpdate} saving={busy.settings}
              saveAutoUpdate={autoUpdate => setPreferences({ ...preferences, autoUpdate })} />
          {/if}
          {#if open && errors.settings}
            <p class="settings-note error" role="status">{local('Could not save settings.', '无法保存设置。')}</p>
          {/if}
        </div>
      </section>
    {/each}
  </div>
</section>
