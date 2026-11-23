<script lang="ts">
  import { onMount } from 'svelte';
  import { backend } from './api';
  import type { UpdateStatus } from './api';
  import { local, localized } from './language.svelte';

  let { autoUpdate, saving, saveAutoUpdate }: {
    autoUpdate: boolean; saving: boolean; saveAutoUpdate: (enabled: boolean) => void;
  } = $props();

  let status = $state<UpdateStatus>();
  let pending = $state(false);
  let requestFailed = $state(false);
  let working = $derived(pending || status?.phase === 'checking' || status?.phase === 'downloading' || status?.phase === 'installing');
  const dateTime = localized(locale => new Intl.DateTimeFormat(locale, { dateStyle: 'short', timeStyle: 'short' }));
  const phases: Record<UpdateStatus['phase'], [string, string]> = {
    idle: ['Awaiting update check', '等待检查更新'],
    checking: ['Checking for updates…', '正在检查更新…'],
    current: ['Up to date', '已是最新版'],
    available: ['Update available', '有可用更新'],
    downloading: ['Downloading update…', '正在下载更新…'],
    ready: ['Ready to update', '更新已就绪'],
    installing: ['Preparing to restart…', '正在准备重启…']
  };
  const failures: Record<NonNullable<UpdateStatus['failure']>, [string, string]> = {
    network: ['Could not reach GitHub Releases.', '无法连接 GitHub Releases。'],
    missingManifest: ['The release has no update manifest yet.', '发布版暂未提供更新清单。'],
    invalidManifest: ['The update manifest is invalid.', '更新清单无效。'],
    download: ['Could not download the update.', '无法下载更新。'],
    invalidArchive: ['The update ZIP failed verification. Download it again.', '更新包校验失败，请重新下载。'],
    storage: ['Could not write the update cache.', '无法写入更新缓存。'],
    installDirectory: ['The app folder or a package file is read-only.', '应用目录或包内文件不可写。'],
    installFailed: ['Could not start the update installer.', '无法启动更新程序。']
  };
  const results: Record<NonNullable<UpdateStatus['lastInstall']>, [string, string]> = {
    updated: ['Update installed.', '更新已安装。'],
    installFailed: ['The update failed. You are running the previous version.', '安装失败，已恢复旧版。'],
    rollbackFailed: ['The update could not restore all files. Backups are in the app folder.',
      '部分文件恢复失败，备份位于应用目录。']
  };

  onMount(() => {
    let active = true;
    let changed = false;
    let stop: (() => void) | undefined;
    void backend.updateStatusChanged(next => {
      if (active)
      {
        changed = true;
        status = next;
      }
    }).catch(() => undefined).then(async unsubscribe => {
      if (!active)
      {
        unsubscribe?.();
        return;
      }
      stop = unsubscribe;
      const snapshot = await backend.updateStatus();
      if (active && !changed) status = snapshot;
    }).catch(() => {
      if (active) requestFailed = true;
    });
    return () => {
      active = false;
      stop?.();
    };
  });

  async function run(command: () => Promise<void>)
  {
    if (pending) return;
    pending = true;
    requestFailed = false;
    try { await command(); }
    catch { requestFailed = true; }
    finally { pending = false; }
  }
</script>

<div class="settings-item settings-fact">
  <span>{local('Current version', '当前版本')}</span>
  <span>{status ? 'v' + status.version : '—'}</span>
</div>
{#if status?.availableVersion}
  <div class="settings-item settings-fact">
    <span>{local('Available version', '可用版本')}</span>
    <span>v{status.availableVersion}</span>
  </div>
{/if}
<div class="settings-item settings-fact">
  <span>{local('Last update check', '最近检查')}</span>
  <span>{status?.checkedAt ? dateTime().format(status.checkedAt) : '—'}</span>
</div>
<div class="settings-item">
  <label for="auto-update">{local('Automatic updates', '自动更新')}</label>
  <input id="auto-update" class="settings-check" type="checkbox" checked={autoUpdate} disabled={saving}
    onchange={event => saveAutoUpdate(event.currentTarget.checked)} />
  <p class="settings-note">
    {local('Checks and downloads in the background. Install with Restart and update.',
      '后台检查并下载，点击「重启并更新」后安装。')}
  </p>
</div>
<div class="settings-item">
  <div class="settings-update-actions">
    <button class="settings-action" type="button" onclick={() => run(backend.checkUpdates)} disabled={working}>
      {local('Check for updates', '检查更新')}
    </button>
    {#if status?.canInstall && status.phase === 'available'}
      <button class="settings-action" type="button" onclick={() => run(backend.downloadUpdate)} disabled={working}>
        {local('Download update', '下载更新')}
      </button>
    {:else if status?.phase === 'ready'}
      <button class="settings-action" type="button" onclick={() => run(backend.installUpdate)} disabled={working}>
        {local('Restart and update', '重启并更新')}
      </button>
    {/if}
    <button class="settings-action" type="button" onclick={() => run(backend.openUpdateReleases)} disabled={pending}>
      {local('GitHub releases', 'GitHub 发布页')}
    </button>
  </div>
  <div class="settings-note" aria-live="polite">
    {#if status?.failure}
      <p class="settings-note error">{local(...failures[status.failure])}</p>
    {:else if status}
      <p class="settings-note">{local(...phases[status.phase])}</p>
    {/if}
    {#if status?.lastInstall}
      <p class="settings-note" class:error={status.lastInstall !== 'updated'}>{local(...results[status.lastInstall])}</p>
    {/if}
    {#if requestFailed}
      <p class="settings-note error">{local('Could not complete the update action.', '无法执行更新操作。')}</p>
    {/if}
  </div>
</div>
