<script lang="ts">
  import { onMount } from 'svelte';
  import { backend } from './api';
  import type { ProxyStatus } from './api';
  import { local } from './language.svelte';

  const sources: Record<ProxyStatus['source'], [string, string]> = {
    custom: ['Custom proxy', '自定义代理'],
    launchArgument: ['Launch argument', '启动参数'],
    environment: ['Environment variables', '环境变量'],
    windows: ['Windows settings', 'Windows 设置']
  };
  const messages = {
    saved: ['Proxy settings saved.', '代理设置已保存。'],
    loadFailed: ['Could not load proxy settings.', '无法读取代理设置。'],
    saveFailed: ['Could not save proxy settings.', '无法保存代理设置。'],
    invalidUrl: ['Enter a proxy address such as http://127.0.0.1:7890.', '请输入代理地址，例如 http://127.0.0.1:7890。'],
    unsupportedScheme: ['Use HTTP, HTTPS, SOCKS5 or SOCKS5H.', '请使用 HTTP、HTTPS、SOCKS5 或 SOCKS5H。'],
    credentialsUnsupported: ['For a proxy with credentials, use the HTTPS_PROXY environment variable.',
      '需要账号密码的代理请通过 HTTPS_PROXY 环境变量配置。'],
    invalidBypass: ['Enter domains, IP addresses, CIDR ranges such as 10.0.0.0/8, or *.',
      '绕过项可填域名、IP 地址、CIDR 网段（如 10.0.0.0/8）或 *。'],
    bypassTooLong: ['The bypass list can contain up to 2,000 characters.', '绕过列表最多 2,000 个字符。']
  } satisfies Record<string, [string, string]>;

  let current = $state<ProxyStatus>();
  let proxyUrl = $state('');
  let noProxy = $state('');
  let saving = $state(false);
  let message = $state<keyof typeof messages | null>(null);
  let changed = $derived(current && (proxyUrl !== current.proxyUrl || noProxy !== current.noProxy));

  function apply(settings: ProxyStatus)
  {
    current = settings;
    proxyUrl = settings.proxyUrl;
    noProxy = settings.noProxy;
  }

  onMount(() => {
    let active = true;
    void backend.proxySettings().then(settings => {
      if (active) apply(settings);
    }).catch(() => {
      if (active) message = 'loadFailed';
    });
    return () => { active = false; };
  });

  async function save(event: SubmitEvent)
  {
    event.preventDefault();
    if (!changed || saving) return;
    message = null;
    saving = true;
    try
    {
      apply(await backend.saveProxySettings({ proxyUrl, noProxy }));
      message = 'saved';
    }
    catch (error)
    {
      message = typeof error === 'string' && Object.hasOwn(messages, error) ? error as keyof typeof messages : 'saveFailed';
    }
    finally { saving = false; }
  }
</script>

<form onsubmit={save}>
  <div class="settings-item settings-fact">
    <span>{local('Proxy source', '代理来源')}</span>
    <span>{current ? local(...sources[current.source]) : '—'}</span>
  </div>
  <div class="settings-item settings-proxy-field">
    <label for="proxy-url">{local('Proxy address', '代理地址')}</label>
    <input id="proxy-url" type="text" bind:value={proxyUrl} oninput={() => { message = null; }}
      placeholder="http://127.0.0.1:7890" autocomplete="off" spellcheck="false" disabled={!current || saving}
      aria-describedby="proxy-note" />
    <p id="proxy-note" class="settings-note">
      {local('For Lite’s limit and update requests. Leave empty to use launch arguments, environment variables and Windows settings.',
        '用于 Lite 的额度查询和更新。留空时依次使用启动参数、环境变量和 Windows 代理。')}
    </p>
  </div>
  <div class="settings-item settings-proxy-field">
    <label for="proxy-bypass">{local('Bypass proxy for', '绕过代理')}</label>
    <input id="proxy-bypass" type="text" bind:value={noProxy} oninput={() => { message = null; }}
      placeholder="example.com, 10.0.0.0/8" autocomplete="off" spellcheck="false" disabled={!current || saving}
      aria-describedby="proxy-bypass-note" />
    <p id="proxy-bypass-note" class="settings-note">
      {local('Comma-separated hosts or CIDR ranges. Local addresses connect directly.',
        '用逗号分隔主机或 CIDR 网段。本机地址直连。')}
    </p>
  </div>
  <div class="settings-item settings-proxy-save">
    <button class="settings-action" type="submit" disabled={!changed || saving}>
      {saving ? local('Saving…', '正在保存…') : local('Save proxy settings', '保存代理设置')}
    </button>
    {#if message}
      <p class="settings-note" class:error={message !== 'saved'} role="status">{local(...messages[message])}</p>
    {/if}
  </div>
</form>
