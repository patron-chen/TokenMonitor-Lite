<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { backend, defaultPreferences, emptySessions, pageSize, toolIds } from './api';
  import type { Activity, Appearance, Backdrop, CollectionStatus, Detail, Period,
    Preferences, Quotas, Session, SessionKey, SettingsSection, StatisticsRange, Summary, UiLanguage, View, ViewState } from './api';
  import { cacheHitLabel, callLabel, compact, cost, effortTitle, icons, local, modelIcon, number, requestPerformanceParts,
    sessionActivityParts, sessionKey, sessionModelLabel, sessionTitle, shortSessionTime, splits,
    statisticsRangeTabLabels, time, toolInfo, viewLabel, views } from './display';
  import { isChinese, setLanguage } from './language.svelte';
  import BreakdownRows from './BreakdownRows.svelte';
  import HomeHistory from './HomeHistory.svelte';
  import QuotaSection from './QuotaSection.svelte';
  import QuotaView from './QuotaView.svelte';
  import SessionRow from './SessionRow.svelte';
  import SessionContext from './SessionContext.svelte';
  import SessionModel from './SessionModel.svelte';
  import Pager from './Pager.svelte';
  import SettingsPanel from './SettingsPanel.svelte';
  import './styles.css';

  const initial: ViewState = {
    period: 'today', view: 'home', fromHome: false, tool: null, query: '',
    searchOpen: false, page: 0, scroll: 0, selection: null, sort: 'newest',
    detailPage: 0, detailScroll: 0, openExchanges: [], expanded: null, toolMode: 'tokens'
  };
  let route = $state<ViewState>({ ...initial });
  let summary = $state<Summary | null>(null);
  let sessions = $state(emptySessions);
  let detail = $state<Detail | null>(null);
  let ready = $state(false);
  let backdrop = $state<Backdrop>('none');
  let appearance = $state<Appearance>({ opacity: 88, fontScale: 100 });
  let preferences = $state<Preferences>(defaultPreferences);
  let systemLanguage = $state<UiLanguage>();
  let settingsOpen = $state(false);
  let settingsSection = $state<SettingsSection | null>('appearance');
  let settingsButton: HTMLButtonElement | undefined = $state();
  let settingsBack: HTMLButtonElement | undefined = $state();
  let settingsSaving = $state(false);
  let settingsError = $state(false);
  let appearanceError = $state(false);
  let collection = $state<CollectionStatus | null>(null);
  let collectionLoading = $state(false);
  let collectionError = $state(false);
  let collectionRequest = 0;
  let activity = $state<Activity | null>(null);
  let activityLoading = false;
  let activityRequest = 0;
  let activityError = $state(false);
  let quotas = $state<Quotas | null>(null);
  let quotaLoading = $state(false);
  let quotaRequest = 0;
  let lastQuotaCheck = $state(0);
  let now = $state(Date.now());
  let copyResult = $state<{ key: string; copied: boolean } | null>(null);
  let error = $state<[string, string] | null>(null);
  let detailError = $state<[string, string] | null>(null);
  let loadingDetail = $state(false);
  let viewMenu = $state(false);
  let summaryRequest = 0;
  let listRequest = 0;
  let detailRequest = 0;
  let rememberTimer: ReturnType<typeof setTimeout> | undefined;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;
  let holdTimer: ReturnType<typeof setTimeout> | undefined;
  let held = false;
  let viewport: HTMLElement | undefined = $state();
  let heroNumber: HTMLDivElement | undefined = $state();
  let disclosure: HTMLButtonElement | undefined = $state();
  let searchButton: HTMLButtonElement | undefined = $state();
  let searchInput: HTMLInputElement | undefined = $state();
  let backButton: HTMLButtonElement | undefined = $state();
  let filterStrip: HTMLDivElement | undefined = $state();
  let filterOverflow = $state(false);
  let filterStart = $state(true);
  let filterEnd = $state(true);
  let periodSlot = $derived(route.period === 'today' ? 0 : (route.period === 'allTime' ? 2 : 1));
  let middlePeriod = $derived<'month' | StatisticsRange>(route.view === 'sessions' ? 'month' : preferences.statisticsRange);
  let hasSessions = $derived(route.period === 'today' || route.period === 'month' || route.period === 'allTime');
  let maxSession = $derived(Math.max(1, ...sessions.rows.map(row => row.usage.tokens)));
  let maxExchange = $derived(Math.max(1, ...(detail?.exchanges.map(exchange => exchange.tokens) ?? [])));
  let activeTool = $derived(summary?.tools.find(row => row.key === route.expanded));
  let sessionCount = $derived(sessions.rows.length);
  let sessionMode = $derived(route.view === 'sessions' || !!route.selection);
  let count = $derived(summary?.updatedAt ? (sessionMode ? compact : number)(summary.usage.tokens) : '—');
  let antigravityWithoutUsage = $derived(summary?.updatedAt && summary.antigravityLocal &&
    !summary.tools.some(row => row.key === 'antigravity'));
  let quotaVisible = $derived(!settingsOpen && !route.selection && (route.view === 'home' || route.view === 'limits'));
  let shownQuotas = $derived(quotas && {
    ...quotas, tools: quotas.tools.filter(provider => preferences.quotas.tools[provider.tool])
  });

  $effect(() => setLanguage(preferences.language, systemLanguage));

  $effect(() => {
    document.documentElement.style.setProperty('--ui-text-scale', String(appearance.fontScale / 100));
    if (count) fitTotalNumber();
  });

  $effect(() => {
    if (!ready) return;
    // Give the restored panel a frame before showing the transparent host.
    let frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(reveal);
    });
    // WebView2 can throttle frame callbacks while its host is hidden.
    const fallback = setTimeout(reveal, 120);
    return cancelReveal;

    function cancelReveal()
    {
      cancelAnimationFrame(frame);
      clearTimeout(fallback);
    }

    function reveal()
    {
      cancelReveal();
      void backend.detailsReady().catch(() => {});
    }
  });

  $effect(() => {
    if (!ready || !quotaVisible || quotaLoading || !toolIds.some(tool => preferences.quotas.tools[tool])) return;
    const interval = preferences.quotas.refreshMinutes * 60_000;
    const elapsed = lastQuotaCheck ? Math.max(0, Date.now() - lastQuotaCheck) : 0;
    const timer = setTimeout(() => void loadQuotas(), Math.max(1_000, interval - elapsed));
    return () => clearTimeout(timer);
  });

  onMount(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    let unsubscribeForecast: (() => void) | undefined;
    backend.forecastUpdated(snapshot => {
      if (ready && quotaVisible) applyQuotas(snapshot);
    }).then(unlisten => {
      if (disposed) unlisten();
      else unsubscribeForecast = unlisten;
    }).catch(() => {});
    backend.updated(period => {
      if (ready && period === 'today' && settingsOpen && settingsSection === 'collection') void loadCollection();
      if (ready && period === route.period) void refreshScreen(true);
      else if (ready && period === 'today' && route.view === 'home') void refreshActivityToday();
    }).then(unlisten => {
      if (disposed) unlisten();
      else unsubscribe = unlisten;
    }).catch(() => {});
    void initialize();
    const displayClock = setInterval(() => {
      if (ready) now = Date.now();
    }, 30_000);
    return () => {
      disposed = true;
      unsubscribe?.();
      unsubscribeForecast?.();
      clearInterval(displayClock);
      [rememberTimer, searchTimer, hoverTimer, holdTimer].forEach(clearTimeout);
    };
  });

  async function initialize()
  {
    try
    {
      const previous = await backend.view();
      preferences = { ...defaultPreferences, ...previous.preferences };
      systemLanguage = previous.systemLanguage;
      if (previous.view)
      {
        route = { ...initial, ...previous.view };
        if (periodSlot === 1) route.period = middlePeriod;
        if (!hasSessions) route.selection = null;
      }
      backdrop = previous.backdrop;
      appearance = previous.appearance;
      quotas = previous.cachedQuotas ?? null;
      lastQuotaCheck = quotas?.checkedAt ?? 0;
      ready = true;
      loadViewExtras(true);
      if (route.period !== 'today') await backend.refresh(route.period);
      await refreshScreen();
      await tick();
      if (viewport) viewport.scrollTop = route.selection ? route.detailScroll : route.scroll;
    }
    catch
    {
      ready = true;
      error = ['Could not open the local usage view.', '无法打开本地用量视图。'];
    }
  }

  function loadViewExtras(refreshQuotas = false)
  {
    if (route.selection) return;
    if (route.view === 'home') void loadActivity();
    if (quotaVisible) void loadQuotas(refreshQuotas);
  }

  function localDay()
  {
    const now = new Date();
    return [now.getFullYear(), String(now.getMonth() + 1).padStart(2, '0'), String(now.getDate()).padStart(2, '0')].join('-');
  }

  async function loadActivity(force = false)
  {
    if (activityLoading || (!force && activity?.today === localDay())) return;
    const request = ++activityRequest;
    activityLoading = true;
    activityError = false;
    try
    {
      const data = await backend.activity(force);
      if (request === activityRequest) activity = data;
    }
    catch { if (request === activityRequest) activityError = true; }
    finally { if (request === activityRequest) activityLoading = false; }
  }

  function updateActivityToday(data: Summary)
  {
    if (!activity) return;
    if (activity.today !== localDay()) void loadActivity();
    else if (data.updatedAt) activity.todayTokens = data.usage.tokens;
  }

  async function refreshActivityToday()
  {
    if (!activity) return;
    try { updateActivityToday(await backend.summary('today')); }
    catch { /* Keep the last completed activity view. */ }
  }

  async function recordBankedResetPanel(request: number, readFailed: boolean)
  {
    // Check the committed DOM so a positive backend count can be compared with the row on screen.
    await tick();
    if (request !== quotaRequest || !quotaVisible) return;
    const count = shownQuotas?.tools.find(provider => provider.tool === 'codex')?.bankedReset?.availableCount ?? null;
    const rendered = !!viewport?.querySelector('.quota-banked');
    await backend.bankedResetPanel(route.view === 'limits', preferences.quotas.tools.codex, readFailed, count, rendered).catch(() => {});
  }

  async function loadQuotas(force = false)
  {
    if (!quotaVisible) return;
    if (!toolIds.some(tool => preferences.quotas.tools[tool]))
    {
      quotaRequest++;
      quotas = null;
      quotaLoading = false;
      void recordBankedResetPanel(quotaRequest, false);
      return;
    }
    if (quotaLoading && !force) return;
    const request = ++quotaRequest;
    let readFailed = false;
    quotaLoading = true;
    try
    {
      const data = await backend.quotas(force);
      if (request !== quotaRequest) return;
      applyQuotas(data);
    }
    catch
    {
      if (request !== quotaRequest) return;
      lastQuotaCheck = Date.now();
      readFailed = true;
    }
    finally
    {
      if (request === quotaRequest)
      {
        quotaLoading = false;
        void recordBankedResetPanel(request, readFailed);
      }
    }
  }

  function applyQuotas(data: Quotas)
  {
    // A foreground request can finish delivering after the tray has published newer usage.
    if (quotas && Math.max(data.observedAt, data.checkedAt ?? 0) < Math.max(quotas.observedAt, quotas.checkedAt ?? 0)) return;
    quotas = data;
    now = Date.now();
    lastQuotaCheck = data.checkedAt ?? now;
  }

  async function loadCollection()
  {
    const request = ++collectionRequest;
    collectionLoading = true;
    collectionError = false;
    try
    {
      const data = await backend.collection();
      if (request === collectionRequest) collection = data;
    }
    catch { if (request === collectionRequest) collectionError = true; }
    finally { if (request === collectionRequest) collectionLoading = false; }
  }

  async function refreshCollection()
  {
    try
    {
      await backend.refresh('today');
      await loadCollection();
    }
    catch { collectionError = true; }
  }

  function selectSettingsSection(section: SettingsSection | null)
  {
    settingsSection = section;
    if (section === 'collection') void loadCollection();
  }

  async function toggleSettings()
  {
    if (!settingsOpen)
    {
      captureScroll();
      viewMenu = false;
      settingsOpen = true;
      selectSettingsSection(settingsSection);
      await tick();
      settingsBack?.focus();
    }
    else
    {
      settingsOpen = false;
      loadViewExtras();
      await tick();
      if (viewport) viewport.scrollTop = route.selection ? route.detailScroll : route.scroll;
      updateFilterOverflow();
      fitTotalNumber();
      settingsButton?.focus({ preventScroll: true });
    }
  }

  async function savePreferences(next: Preferences)
  {
    if (settingsSaving) return;
    const previous = $state.snapshot(preferences);
    settingsSaving = true;
    settingsError = false;
    preferences = next;
    try
    {
      await backend.preferences($state.snapshot(next), route.period);
      if (periodSlot === 1 && route.period !== middlePeriod) await choosePeriod(middlePeriod, true);
      if (toolIds.some(tool => next.collection.tools[tool] !== previous.collection.tools[tool]))
      {
        activityRequest++;
        activityLoading = false;
        activity = null;
        void refreshScreen();
        if (!settingsOpen) loadViewExtras();
      }
      if (settingsOpen && settingsSection === 'collection') void loadCollection();
      if (quotaVisible && toolIds.some(tool => next.quotas.tools[tool] !== previous.quotas.tools[tool]))
      {
        void loadQuotas(true);
      }
    }
    catch
    {
      preferences = previous;
      settingsError = true;
    }
    finally { settingsSaving = false; }
  }

  async function saveAppearance()
  {
    appearanceError = false;
    try { await backend.appearance($state.snapshot(appearance)); }
    catch { appearanceError = true; }
  }

  function fitTotalNumber()
  {
    const number = heroNumber;
    if (!number) return;
    number.style.fontSize = '';
    if (number.clientWidth > 0 && number.scrollWidth > number.clientWidth)
    {
      // Match the original total's width fit while leaving its base size fixed.
      const base = parseFloat(getComputedStyle(number).fontSize);
      const scale = Math.max(0.5, number.clientWidth / number.scrollWidth);
      number.style.fontSize = Math.floor(base * scale) + 'px';
    }
  }

  function remember(immediate = false)
  {
    if (!ready) return;
    clearTimeout(rememberTimer);
    if (immediate) return backend.remember($state.snapshot(route)).catch(() => {});
    rememberTimer = setTimeout(() => void backend.remember($state.snapshot(route)).catch(() => {}), 100);
  }

  function captureScroll()
  {
    if (!viewport) return;
    if (route.selection) route.detailScroll = viewport.scrollTop;
    else route.scroll = viewport.scrollTop;
    remember();
  }

  async function dismiss()
  {
    captureScroll();
    await remember(true);
    await backend.dismiss();
  }

  async function refreshScreen(fromUpdate = false)
  {
    const request = ++summaryRequest;
    const period = route.period;
    try
    {
      const data = await backend.summary(period);
      if (request !== summaryRequest || period !== route.period) return;
      const changed = summary?.updatedAt !== data.updatedAt;
      summary = data;
      if (period === 'today') updateActivityToday(data);
      error = null;
      if (fromUpdate && !changed) return;
      if (route.selection && hasSessions) await loadDetail(fromUpdate);
      else if (route.view === 'home' || route.view === 'sessions') await loadSessions();
    }
    catch
    {
      if (request === summaryRequest && period === route.period) error = ['Could not read local usage.', '无法读取本地用量。'];
    }
  }

  async function loadSessions()
  {
    const request = ++listRequest;
    if (!hasSessions)
    {
      sessions = emptySessions;
      return;
    }
    const home = route.view === 'home';
    const period = route.period;
    const data = await backend.sessions(period, home ? null : route.tool, home ? '' : route.query, home ? 0 : route.page, home);
    if (request !== listRequest || period !== route.period) return;
    if (!home && route.tool && !data.tools.includes(route.tool))
    {
      route.tool = null;
      route.page = 0;
      if (data.total > 0) return loadSessions();
    }
    if (!home && route.page > 0 && route.page * pageSize >= data.total)
    {
      route.page = Math.max(0, Math.ceil(data.total / pageSize) - 1);
      return loadSessions();
    }
    sessions = data;
    now = Date.now();
    await tick();
    updateFilterOverflow();
  }

  async function copySessionId(session: Session)
  {
    const value = session.nativeId ?? session.id;
    const key = sessionKey(session);
    try
    {
      await navigator.clipboard.writeText(value);
      copyResult = { key, copied: true };
    }
    catch
    {
      copyResult = { key, copied: false };
    }
  }

  async function loadDetail(keepPosition = false)
  {
    if (!route.selection) return;
    const request = ++detailRequest;
    const offset = keepPosition ? viewport?.scrollTop ?? route.detailScroll : route.detailScroll;
    const selection = route.selection;
    const period = route.period;
    detailError = null;
    loadingDetail = !keepPosition;
    try
    {
      const data = await backend.detail(period, selection, route.sort, route.detailPage);
      if (request !== detailRequest) return;
      detail = data;
      if (!data && summary?.updatedAt)
      {
        detailError = ['No activity in this period.', '该周期内没有这条会话的用量。'];
      }
      await tick();
      if (viewport && request === detailRequest) viewport.scrollTop = offset;
    }
    catch
    {
      if (request === detailRequest) detailError = ['Could not read the local transcript.', '无法读取本地转录。'];
    }
    finally
    {
      if (request === detailRequest) loadingDetail = false;
    }
  }

  function resetViewport()
  {
    route.scroll = 0;
    route.detailScroll = 0;
    if (viewport) viewport.scrollTop = 0;
  }

  async function choosePeriod(period: Period, rememberImmediately = false)
  {
    if (period === route.period) return;
    route.period = period;
    route.page = 0;
    route.detailPage = 0;
    route.openExchanges = [];
    summary = null;
    sessions = emptySessions;
    detail = null;
    if (!hasSessions) route.selection = null;
    resetViewport();
    void remember(rememberImmediately);
    try
    {
      if (period !== 'today') await backend.refresh(period);
      await refreshScreen();
    }
    catch
    {
      error = ['Could not refresh this period.', '无法刷新该周期。'];
    }
  }

  async function chooseView(view: View, fromHome = false, expanded: string | null = null)
  {
    viewMenu = false;
    if (view === route.view && !route.selection) return;
    route.view = view;
    route.fromHome = fromHome;
    route.selection = null;
    route.expanded = expanded;
    route.page = 0;
    if (view !== 'sessions')
    {
      route.searchOpen = false;
      route.query = '';
    }
    detail = null;
    detailRequest++;
    loadViewExtras();
    if (periodSlot === 1 && route.period !== middlePeriod) await choosePeriod(middlePeriod, true);
    else
    {
      resetViewport();
      void remember(true);
      if (view === 'home' || view === 'sessions') await loadSessions().catch(showListError);
    }
    if (fromHome)
    {
      await tick();
      backButton?.focus({ preventScroll: true });
    }
  }

  function cycleView()
  {
    if (held) { held = false; return; }
    const next = views[(views.indexOf(route.view) + 1) % views.length];
    void chooseView(next);
  }

  function startHold(event: PointerEvent)
  {
    if (event.button !== 0) return;
    clearTimeout(holdTimer);
    held = false;
    holdTimer = setTimeout(() => { held = true; void setViewMenu(true, true); }, 420);
  }

  async function setViewMenu(open: boolean, focus = false)
  {
    clearTimeout(hoverTimer);
    viewMenu = open;
    if (open && focus)
    {
      await tick();
      document.querySelector<HTMLButtonElement>('#view-menu [aria-checked="true"]')?.focus();
    }
  }

  function closeHover()
  {
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(() => { viewMenu = false; }, 160);
  }

  function menusKeydown(event: KeyboardEvent, restore: HTMLElement | undefined)
  {
    const menu = event.currentTarget as HTMLElement;
    const buttons = Array.from(menu.querySelectorAll<HTMLButtonElement>('button'));
    if (event.key === 'Escape')
    {
      event.preventDefault();
      event.stopPropagation();
      viewMenu = false;
      restore?.focus();
      return;
    }
    if (event.key === 'Tab')
    {
      viewMenu = false;
      return;
    }
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const direction = event.key === 'ArrowDown' || event.key === 'ArrowRight' ? 1 :
      (event.key === 'ArrowUp' || event.key === 'ArrowLeft' ? -1 : 0);
    if (direction || event.key === 'Home' || event.key === 'End')
    {
      event.preventDefault();
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 :
        (index + buttons.length + direction) % buttons.length;
      buttons[next]?.focus();
    }
  }

  function outside(event: MouseEvent)
  {
    const target = event.target as Element | null;
    if (!target?.closest('[data-view-control]')) viewMenu = false;
  }

  function escape(event: KeyboardEvent)
  {
    if (event.key !== 'Escape') return;
    if (viewMenu)
    {
      viewMenu = false;
      disclosure?.focus();
    }
    else if (settingsOpen)
    {
      event.preventDefault();
      void toggleSettings();
    }
    else if (route.selection) void closeSession();
    else if (route.searchOpen) void toggleSearch();
  }

  function expand(key: string)
  {
    route.expanded = route.expanded === key ? null : key;
    remember();
  }

  function showListError()
  {
    error = ['Could not list sessions.', '无法读取会话列表。'];
  }

  async function changeList(page: number, tool = route.tool)
  {
    route.page = page;
    route.tool = tool;
    resetViewport();
    remember();
    await loadSessions().catch(showListError);
  }

  async function toggleSearch()
  {
    route.searchOpen = !route.searchOpen;
    if (route.searchOpen)
    {
      await tick();
      searchInput?.focus();
    }
    else
    {
      if (route.query)
      {
        route.query = '';
        void changeList(0);
      }
      searchButton?.focus();
    }
    remember();
  }

  function search(value: string)
  {
    route.query = value;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void changeList(0), 150);
  }

  async function openSession(row: Session)
  {
    copyResult = null;
    route.scroll = viewport?.scrollTop ?? 0;
    route.selection = { id: row.id, tool: row.tool };
    route.openExchanges = [];
    route.detailPage = 0;
    route.detailScroll = 0;
    detail = null;
    remember();
    void loadDetail();
    await tick();
    backButton?.focus({ preventScroll: true });
  }

  async function closeSession()
  {
    const oldSession = route.selection;
    const offset = route.scroll;
    route.selection = null;
    detail = null;
    detailRequest++;
    remember();
    loadViewExtras();
    await loadSessions().catch(showListError);
    await tick();
    if (viewport) viewport.scrollTop = offset;
    if (oldSession)
    {
      const key = CSS.escape(sessionKey(oldSession));
      viewport?.querySelector<HTMLElement>('[data-session-key="' + key + '"]')?.focus({ preventScroll: true });
    }
  }

  function detailPage(page: number)
  {
    route.detailPage = page;
    route.detailScroll = 0;
    remember();
    void loadDetail();
  }

  function toggleExchange(id: number)
  {
    route.openExchanges = route.openExchanges.includes(id) ?
      route.openExchanges.filter(value => value !== id) : [...route.openExchanges, id];
    remember();
  }

  function updateFilterOverflow()
  {
    if (!filterStrip) return;
    filterOverflow = filterStrip.scrollWidth > filterStrip.clientWidth + 2;
    filterStart = filterStrip.scrollLeft <= 1;
    filterEnd = filterStrip.scrollLeft + filterStrip.clientWidth >= filterStrip.scrollWidth - 1;
  }

  function scrollTools(direction: number)
  {
    filterStrip?.scrollBy({ left: direction * filterStrip.clientWidth * 0.75, behavior: 'smooth' });
  }

  async function manualRefresh()
  {
    try
    {
      await backend.refresh(route.period);
      if (!route.selection && route.view === 'home') void loadActivity(true);
      if (quotaVisible) void loadQuotas(true);
      await refreshScreen(true);
    }
    catch
    {
      error = ['Could not refresh usage.', '无法刷新用量。'];
    }
  }

  function detailNotice(status: Detail['status']): string
  {
    if (status === 'missing') return local('The local transcript is unavailable.', '本机转录已不可用。');
    if (status === 'unreadable') return local('Could not read the local transcript.', '无法读取本地转录。');
    if (status === 'partial') return local('The transcript provides partial turn data.', '源记录仅提供部分逐轮明细。');
    return '';
  }
</script>

<svelte:window onclick={outside} onkeydown={escape} onresize={() => { updateFilterOverflow(); fitTotalNumber(); }} />

<main class="shell" class:settings-open={settingsOpen} data-backdrop={backdrop} style:--surface-opacity={appearance.opacity / 100}>
  <header class="titlebar" data-tauri-drag-region>
    {#if settingsOpen}
      <button class="back-button settings-back" bind:this={settingsBack} type="button"
        onclick={toggleSettings} aria-label={local('Back to usage', '返回用量')}>
        <span class="icon" style:--icon={'url(' + icons.back + ')'} aria-hidden="true"></span>
        {local('Settings', '设置')}
      </button>
    {:else}
    <div class="brand" data-tauri-drag-region title="Token Monitor Lite">
      <span data-tauri-drag-region>Σ</span><span class="live-dot" class:online={summary?.updatedAt && !summary.failure} aria-hidden="true"></span>
    </div>
    <nav class="tabs" aria-label={local('Period tabs', '统计周期')} style:--period-index={periodSlot}>
      <span class="tab-indicator" aria-hidden="true"></span>
      <button class="tab" aria-pressed={periodSlot === 0} onclick={() => choosePeriod('today')}>DAY</button>
      <button class="tab" aria-pressed={periodSlot === 1} onclick={() => choosePeriod(middlePeriod)}>
        {middlePeriod === 'month' ? 'MONTH' : statisticsRangeTabLabels[middlePeriod]}
      </button>
      <button class="tab" aria-pressed={periodSlot === 2} onclick={() => choosePeriod('allTime')}>TOTAL</button>
    </nav>
    {/if}
    <div class="window-controls">
      <button class="window-button" onclick={dismiss} aria-label={local('Minimize to tray', '最小化到托盘')}>−</button>
      <button class="window-button close" onclick={dismiss} aria-label={local('Close to tray', '关闭到托盘')}>×</button>
    </div>
  </header>

  {#if settingsOpen}
    <SettingsPanel bind:appearance {backdrop} {preferences} section={settingsSection} {collection}
      busy={{ settings: settingsSaving, collection: collectionLoading }}
      errors={{ settings: settingsError, appearance: appearanceError, collection: collectionError }}
      selectSection={selectSettingsSection} {saveAppearance} setPreferences={savePreferences} {refreshCollection} />
  {:else}
  <section class="hero" aria-label={local('Usage summary', '用量总览')}>
    <div class="hero-label">
      <span>TOTAL TOKENS</span>
      {#if summary?.updatedAt}
        <time class="updated" datetime={new Date(summary.updatedAt).toISOString()}>
          {local('Updated', '更新于')} {time(summary.updatedAt)}
        </time>
      {/if}
    </div>
    <div class="hero-values">
      <div class="hero-number" bind:this={heroNumber}
        title={sessionMode && summary?.updatedAt ? number(summary.usage.tokens) : undefined}>{count}</div>
      {#if !sessionMode && summary?.updatedAt && summary.usage.tokens >= 1000}
        <span class="hero-compact" aria-hidden="true">≈ {compact(summary.usage.tokens)}</span>
      {/if}
    </div>
    <div class="hero-cost-row">
      <div class="hero-cost" title={local('Estimated API cost', '估算 API 成本')}>{summary?.updatedAt ? cost(summary.usage, true) : '—'}</div>
      {#if route.view === 'sessions' && !route.selection && hasSessions}
        <button class="icon-button search-button" bind:this={searchButton} onclick={toggleSearch}
          aria-expanded={route.searchOpen} aria-label={local('Search sessions', '搜索会话')}>
          <span class="icon" style:--icon={'url(' + icons.search + ')'} aria-hidden="true"></span>
        </button>
      {/if}
    </div>
  </section>

  {#if error || summary?.failure}
    <p class="notice error" role="status">{error ? local(...error) : isChinese() ? '扫描失败，点击刷新重试。' : summary?.failure}</p>
  {/if}
  {#if summary?.antigravityPbOnly}
    <p class="notice" role="status">{local('Antigravity .pb-only records are unavailable.', '仅有 .pb 的 Antigravity 记录暂不可读。')}</p>
  {/if}

  {#if route.selection}
    <div class="detail-head">
      <button class="back-button detail-back" bind:this={backButton} onclick={closeSession}
        aria-label={route.view === 'home' ? local('Back to Home', '返回主页') : local('Back to Sessions', '返回会话')}>
        <span class="icon" style:--icon={'url(' + icons.back + ')'} aria-hidden="true"></span>
        <span class="ellipsis" title={detail ? sessionTitle(detail.session) : undefined}>
          {detail ? sessionTitle(detail.session) : viewLabel(route.view === 'home' ? 'home' : 'sessions')}
        </span>
      </button>
      <button class="sort-button" onclick={() => {
        route.sort = route.sort === 'newest' ? 'tokens' : 'newest';
        detailPage(0);
      }}>{route.sort === 'newest' ? local('↕ Newest', '↕ 最新') : local('↕ Most tokens', '↕ Token 最多')}</button>
    </div>
  {:else if route.view !== 'home' && route.fromHome}
    <div class="detail-head">
      <button class="back-button" bind:this={backButton} onclick={() => chooseView('home')}>
        <span class="icon" style:--icon={'url(' + icons.back + ')'} aria-hidden="true"></span>{viewLabel('home')}
      </button>
    </div>
  {/if}
  {#if route.view === 'sessions' && route.searchOpen && !route.selection && hasSessions}
    <div class="search-field">
      <span class="icon" style:--icon={'url(' + icons.search + ')'} aria-hidden="true"></span>
      <input type="search" bind:this={searchInput} value={route.query} oninput={event => search(event.currentTarget.value)}
        placeholder={local('Search sessions', '搜索会话')} aria-label={local('Search sessions', '搜索会话')} />
    </div>
  {/if}

  <div class="viewport" bind:this={viewport} onscroll={captureScroll}>
    {#if !route.selection && route.view === 'home'}
      <QuotaSection data={shownQuotas} loading={quotaLoading} {now}
        openLimits={tool => chooseView('limits', true, tool ?? null)} />
    {/if}
    {#if route.selection}
      {#if loadingDetail && !detail}<p class="notice">{local('Reading transcript…', '正在读取转录…')}</p>{/if}
      {#if detailError}<p class="notice" role="status">{local(...detailError)}</p>{/if}
      {#if detail}
        {@const session = detail.session}
        {@const provider = toolInfo(session.tool)}
        <div class="session-head" aria-label={sessionTitle(session)}>
          <div class="session-id-line">
            <button class="session-id ellipsis row-meta" type="button" onclick={() => copySessionId(session)}
              title={local('Copy session ID', '复制会话 ID') + ' · ' + (session.nativeId ?? session.id)}>
              <span class="session-id-label">ID</span> {session.nativeId ?? session.id}
            </button>
            {#if copyResult?.key === sessionKey(session)}
              <span class="row-meta" role="status">{copyResult.copied ? local('Copied', '已复制') : local('Could not copy ID', '无法复制 ID')}</span>
            {/if}
          </div>
          <div class="session-head-line">
            <span class="session-detail-model row-meta ellipsis"><span class="icon" style:--icon={'url(' + provider.icon + ')'} aria-hidden="true"></span>
              {provider.name}{#if sessionModelLabel(session) || session.reasoningEffort}{' · '}{/if}<SessionModel {session} />
            </span>
            <div class="session-metrics">
              <b title={number(session.usage.tokens)}>{compact(session.usage.tokens)}</b><span class="row-cost">{cost(session.usage)}</span>
            </div>
          </div>
          <div class="session-head-line">
            <span class="row-meta session-detail-activity">
              {#each sessionActivityParts(session, now) as part, index (part.label)}
                {#if index > 0}{' · '}{/if}<span title={part.label || undefined}>{part.value}</span>
              {/each}
            </span>
            <SessionContext {session} {now} />
          </div>
        </div>
        {#if detailNotice(detail.status)}<p class="notice">{detailNotice(detail.status)}</p>{/if}
        {#each detail.exchanges as exchange (exchange.id)}
          <article class="exchange" class:expanded={route.openExchanges.includes(exchange.id)} style:--row-ink={provider.color}>
            <button class="exchange-button" type="button" aria-expanded={route.openExchanges.includes(exchange.id)}
              onclick={() => toggleExchange(exchange.id)}>
              <span class="chevron" aria-hidden="true"></span>
              <span class="exchange-summary">
                <span class="exchange-title ellipsis" title={exchange.prompt ?? undefined}>{#if exchange.prompt}<span class="you">{local('You ›', '你 ›')}</span> {exchange.prompt}
                {:else}{local('Recorded turns', '已记录轮次')}{/if}</span>
                <span class="row-meta">{[shortSessionTime(exchange.promptAt ?? exchange.lastUsedAt, now),
                  callLabel(exchange.turns.length)].filter(Boolean).join(' · ')}</span>
              </span>
              <b title={number(exchange.tokens)}>{compact(exchange.tokens)}</b>
              <span class="bar" aria-hidden="true"><span style:--share={Math.max(.02, exchange.tokens / maxExchange)}></span></span>
            </button>
            {#if route.openExchanges.includes(exchange.id)}
              <ol class="turns">
                {#each exchange.turns as turn, index}
                  {@const performance = requestPerformanceParts(turn.performance, session.tool)}
                  {@const hit = cacheHitLabel(turn.counts, turn.counts.cacheRead != null || turn.counts.cacheWrite != null)}
                  {@const title = turn.kind === 'attempt' ? local('Attempt', '尝试') :
                    turn.kind === 'compaction' ? local('Compaction', '压缩总结') : local('AI Reply', 'AI 回复')}
                  <li>
                    <div class="turn-head"><span class="turn-role">{title} #{index + 1}</span>
                      <b title={number(turn.tokens)}>{compact(turn.tokens)}</b></div>
                    <div class="row-meta turn-meta">
                      {[shortSessionTime(turn.at, now), provider.name, turn.model].filter(Boolean).join(' · ')}
                      {#if turn.reasoningEffort}{' · '}<span title={effortTitle(turn.reasoningEffort)}>{turn.reasoningEffort}</span>{/if}
                      {#if hit}{' · '}<span title={local('Cache hit', '缓存命中')}>{hit}</span>{/if}
                      {#each performance as metric (metric.label)}
                        {' · '}<span class="request-timing" title={metric.label}>{metric.value}</span>
                      {/each}
                    </div>
                    <div class="turn-breakdown">{splits(turn.counts, compact).map(item => item.label + ' ' + item.value).join(' · ')}</div>
                    {#if turn.tools.length}<div class="row-meta">⊢ {turn.tools.join(', ')}</div>{/if}
                  </li>
                {/each}
              </ol>
            {/if}
          </article>
        {/each}
      {/if}
    {:else if !summary?.updatedAt}
      {#if !error && !summary?.failure}<p class="notice" role="status">{local('Reading local usage…', '正在读取本地用量…')}</p>{/if}
    {:else if route.view === 'limits'}
      <QuotaView data={shownQuotas} loading={quotaLoading} {now} focusTool={route.expanded}
        tracked={preferences.collection.tools} onUpdate={applyQuotas} />
    {:else if route.view === 'tools'}
      <BreakdownRows rows={summary.tools} kind="tool" mode={route.toolMode} expanded={route.expanded} activate={expand} />
      {#if antigravityWithoutUsage}
        <p class="source-note"><span class="icon" style:--icon={'url(' + toolInfo('antigravity').icon + ')'} aria-hidden="true"></span>
          Antigravity <span class="row-meta">{local('No generation records in this period', '本周期暂无生成记录')}</span>
        </p>
      {:else if !summary.tools.length}
        <p class="notice">{local('No recorded usage in this period.', '该周期暂无可读用量。')}</p>
      {/if}
    {:else if route.view === 'models'}
      <BreakdownRows rows={summary.models} toolRows={summary.tools} kind="model" expanded={route.expanded} activate={expand} />
      {#if !summary.models.length}<p class="notice">{local('No recorded usage in this period.', '该周期暂无可读用量。')}</p>{/if}
    {:else if route.view === 'sessions'}
      {#if !hasSessions}
        <p class="notice">{local('Sessions are available for DAY, MONTH and TOTAL.', '会话支持今日、本月和全部。')}</p>
      {:else}
        {#each sessions.rows as row (sessionKey(row))}
          <SessionRow {row} maximum={maxSession} open={openSession} {now} />
        {/each}
        {#if !sessionCount}<p class="notice">{route.query ? local('No matching sessions.', '没有匹配的会话。') : local('No recorded sessions in this period.', '该周期暂无可读会话。')}</p>{/if}
      {/if}
    {:else}
      {#if summary.models.length}
        <section class="home-section">
          <button class="section-link" onclick={() => chooseView('models', true)}>{viewLabel('models')}
            <span class="icon" style:--icon={'url(' + icons.models + ')'} aria-hidden="true"></span>
          </button>
          <div class="home-models">
            {#each summary.models.slice(0, 4) as row (row.key)}
              {@const icon = modelIcon(row.key, summary.tools.find(tool => tool.models.some(model => model.key === row.key))?.key ?? '')}
              <button class="home-model" onclick={() => chooseView('models', true, row.key)} title={row.key + ' · ' + number(row.usage.tokens)}>
                <span class="row-name">
                  {#if icon}<span class="icon" style:--icon={'url(' + icon + ')'} aria-hidden="true"></span>{/if}
                  <span class="ellipsis">{row.key}</span>
                </span>
                <b>{compact(row.usage.tokens)}</b>
                <span class="model-share">{Math.round(row.usage.tokens / Math.max(1, summary.usage.tokens) * 100)}%</span>
              </button>
            {/each}
          </div>
        </section>
      {/if}
      <HomeHistory data={activity} failed={activityError} />
      {#if hasSessions && sessions.rows.length}
        <section class="home-section">
          <button class="section-link" onclick={() => chooseView('sessions', true)}>{viewLabel('sessions')}
            <span class="icon" style:--icon={'url(' + icons.sessions + ')'} aria-hidden="true"></span>
          </button>
          {#each sessions.rows.slice(0, 5) as row (sessionKey(row))}
            <SessionRow {row} maximum={maxSession} open={openSession} {now} preview />
          {/each}
        </section>
      {/if}
    {/if}
  </div>

  {#if route.selection && detail}
    <Pager page={route.detailPage} total={detail.total} change={detailPage} />
  {:else if route.view === 'sessions' && hasSessions}
    {#if sessions.tools.length > 1}
      <div class="filter" role="group" aria-label={local('Session tools', '按工具筛选')}>
        {#if filterOverflow}<button class="filter-arrow" disabled={filterStart} onclick={() => scrollTools(-1)} aria-label={local('Previous tools', '向左滚动')}>‹</button>{/if}
        <div class="filter-strip" bind:this={filterStrip} onscroll={updateFilterOverflow}>
          <button class="filter-pill" aria-pressed={!route.tool} onclick={() => changeList(0, null)}>{local('All', '全部')}</button>
          {#each sessions.tools as tool (tool)}
            <button class="filter-pill" aria-pressed={route.tool === tool} onclick={() => changeList(0, tool)}>
              <span class="icon" style:--icon={'url(' + toolInfo(tool).icon + ')'} aria-hidden="true"></span>{toolInfo(tool).name}
            </button>
          {/each}
        </div>
        {#if filterOverflow}<button class="filter-arrow" disabled={filterEnd} onclick={() => scrollTools(1)} aria-label={local('Next tools', '向右滚动')}>›</button>{/if}
      </div>
    {/if}
    <Pager page={route.page} total={sessions.total} change={page => changeList(page)} />
  {/if}
  {/if}

  <footer class="footer">
    {#if !settingsOpen}
    <div class="view-switcher" role="group" aria-label={local('Views', '视图')} data-view-control
      onpointerenter={() => clearTimeout(hoverTimer)} onpointerleave={closeHover}>
      <button class="view-current" onclick={cycleView} onpointerdown={startHold} onpointerup={() => clearTimeout(holdTimer)}
        onpointercancel={() => clearTimeout(holdTimer)} onpointerleave={() => clearTimeout(holdTimer)}
        oncontextmenu={event => { event.preventDefault(); void setViewMenu(true, true); }}
        title={local('Next view', '切换到下一个视图')}>
        <span class="icon" style:--icon={'url(' + icons[route.view] + ')'} aria-hidden="true"></span>
        {viewLabel(route.view)}
      </button>
      <button class="view-disclosure" bind:this={disclosure} aria-haspopup="menu" aria-controls="view-menu" aria-expanded={viewMenu}
        onpointerenter={event => { if (event.pointerType === 'mouse') void setViewMenu(true); }}
        onclick={event => { if (!(event.detail > 0 && viewMenu)) void setViewMenu(!viewMenu, true); }}
        onkeydown={event => { if (event.key === 'ArrowDown') { event.preventDefault(); void setViewMenu(true, true); } }}
        aria-label={local('Choose view', '选择视图')}><span class="chevron" aria-hidden="true"></span></button>
      {#if viewMenu}
        <div class="glass-menu view-menu" id="view-menu" role="menu" tabindex="-1" aria-label={local('Choose view', '选择视图')}
          onkeydown={event => menusKeydown(event, disclosure)}>
          {#each views as view (view)}
            <button role="menuitemradio" aria-checked={view === route.view} onclick={async () => {
              await chooseView(view);
              disclosure?.focus();
            }}>
              <span class="icon" style:--icon={'url(' + icons[view] + ')'} aria-hidden="true"></span>{viewLabel(view)}
            </button>
          {/each}
        </div>
      {/if}
    </div>
    {#if !route.selection && route.view === 'tools' && activeTool?.models.length && splits(activeTool.usage.counts).length}
      <div class="tool-modes" role="group" aria-label={local('Tool detail', '工具细分')}>
        <button aria-pressed={route.toolMode === 'tokens'} onclick={() => { route.toolMode = 'tokens'; remember(); }}>{local('Tokens', 'Token')}</button>
        <button aria-pressed={route.toolMode === 'models'} onclick={() => { route.toolMode = 'models'; remember(); }}>{viewLabel('models')}</button>
      </div>
    {/if}
    <button class="icon-button refresh" class:working={summary?.refreshing} disabled={summary?.refreshing || !ready}
      aria-label={local('Refresh', '刷新')} title={local('Refresh', '刷新')} onclick={manualRefresh}>
      <span class="icon" style:--icon={'url(' + icons.refresh + ')'} aria-hidden="true"></span>
    </button>
    {/if}
    <button class="icon-button settings-button" bind:this={settingsButton} aria-expanded={settingsOpen}
      aria-controls={settingsOpen ? 'settings-panel' : undefined} disabled={!ready}
      title={local('Settings', '设置')} aria-label={local('Settings', '设置')} onclick={toggleSettings}>
      <span class="icon" style:--icon={'url(' + icons.settings + ')'} aria-hidden="true"></span>
    </button>
  </footer>
</main>
