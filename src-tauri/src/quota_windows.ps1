$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

function Invoke-QuotaRequest($Method, $Uri, $Body, $Headers)
{
    try
    {
        $request = New-Object -ComObject WinHttp.WinHttpRequest.5.1
        $request.SetTimeouts(800, 800, 1000, 2600)
        $request.Open($Method, $Uri, $false)
        $request.Option(6) = $false
        $request.SetAutoLogonPolicy(2)
        foreach ($header in $Headers.GetEnumerator())
        {
            $request.SetRequestHeader($header.Key, $header.Value)
        }
        if ($Method -eq 'POST')
        {
            # The Antigravity service uses a local certificate issued for its own RPC.
            $request.SetProxy(1)
            if ($Uri.StartsWith('https://127.0.0.1:')) { $request.Option(4) = 0x1100 }
            $request.SetRequestHeader('Content-Type', 'application/json')
            $request.SetRequestHeader('Connect-Protocol-Version', '1')
            $request.Send($Body)
        }
        else
        {
            $request.Send()
        }
        if ($request.Status -eq 200) { return ConvertFrom-Json -InputObject $request.ResponseText }
    }
    catch {}
    return $null
}

function Read-Flag([string]$Command, [string]$Name)
{
    $match = [regex]::Match($Command, '(?i)(?:^|\s)--' + $Name + '[=\s]+(?:"([^"]+)"|([^\s"]+))')
    if ($match.Groups[1].Success) { return $match.Groups[1].Value }
    return $match.Groups[2].Value
}

function Get-LoopbackCandidates
{
    $filter = "Name LIKE 'language[_]server%' OR Name LIKE 'language-server%' OR Name LIKE 'agy%' OR Name LIKE 'antigravity-cli%'"
    $processes = Get-CimInstance Win32_Process -Filter $filter -ErrorAction SilentlyContinue
    if (!$processes) { return }
    $listeners = & "$env:SystemRoot\System32\netstat.exe" -ano -p TCP |
        ForEach-Object { [regex]::Match($_, '^\s*TCP\s+(?:127\.0\.0\.1|0\.0\.0\.0):(\d+)\s+\S+\s+LISTENING\s+(\d+)\s*$') } |
        Where-Object Success
    foreach ($process in $processes)
    {
        $line = [string]$process.CommandLine
        $appDir = Read-Flag $line 'app_data_dir'
        $cli = $process.Name -match '^(?:agy|antigravity-cli)(?:\.exe)?$' -or $line -match '(?i)[/\\]antigravity[-_]cli[/\\]'
        if (!$cli -and $appDir -notin @('antigravity', 'antigravity-ide') -and $line -notmatch '(?i)[/\\]antigravity[/\\]')
        {
            continue
        }
        $csrf = Read-Flag $line 'csrf_token'
        if (!$csrf -and (!$cli -or $line -match '(?i)(?:^|\s)--hub(?:[=\s]|$)')) { continue }
        $ports = $listeners | Where-Object { $_.Groups[2].Value -eq [string]$process.ProcessId } |
            ForEach-Object { [int]$_.Groups[1].Value } | Sort-Object -Unique
        $rank = if ($appDir -eq 'antigravity-ide') { 2 } elseif ($cli) { 1 } else { 0 }
        foreach ($port in $ports)
        {
            [pscustomobject]@{ Rank = $rank; Port = $port; Csrf = $csrf }
        }
    }
}

function Get-AntigravityQuota
{
    $metadata = '{"metadata":{"ideName":"antigravity","extensionName":"antigravity","ideVersion":"unknown","locale":"en"}}'
    $summaryBody = if ($env:TOKEN_MONITOR_LITE_FORCE_QUOTA -eq '1') { '{"forceRefresh":true}' } else { '{"forceRefresh":false}' }
    foreach ($candidate in @(Get-LoopbackCandidates | Sort-Object Rank, Port))
    {
        $headers = @{}
        if ($candidate.Csrf) { $headers['X-Codeium-Csrf-Token'] = $candidate.Csrf }
        foreach ($scheme in @('https', 'http'))
        {
            $origin = "${scheme}://127.0.0.1:$($candidate.Port)/exa.language_server_pb.LanguageServerService"
            $summary = Invoke-QuotaRequest 'POST' "$origin/RetrieveUserQuotaSummary" $summaryBody $headers
            $status = Invoke-QuotaRequest 'POST' "$origin/GetUserStatus" $metadata $headers
            $user = $status.userStatus
            $configs = $user.cascadeModelConfigData.clientModelConfigs
            if (!$summary -and !$configs -and $user)
            {
                $models = Invoke-QuotaRequest 'POST' "$origin/GetCommandModelConfigs" $metadata $headers
                $configs = $models.clientModelConfigs
            }
            if ($summary -or $configs)
            {
                $plan = $user.userTier.name
                if (!$plan)
                {
                    $info = $user.planStatus.planInfo
                    $plan = @($info.planDisplayName, $info.displayName, $info.productName, $info.planName, $info.planShortName) |
                        Where-Object { $_ } | Select-Object -First 1
                }
                return @{ summary = $summary; configs = $configs; plan = $plan }
            }
        }
    }
    return $null
}

function Get-CodexBankedReset
{
    if (!$env:TOKEN_MONITOR_LITE_CODEX_AUTH) { return $null }
    try
    {
        $auth = Get-Content -Raw -LiteralPath $env:TOKEN_MONITOR_LITE_CODEX_AUTH | ConvertFrom-Json
        $tokens = if ($auth.tokens) { $auth.tokens } else { $auth }
        $access = ([string]$tokens.access_token).Trim()
        if (!$access) { return $null }
        $headers = @{
            'Authorization' = "Bearer $access"
            'Accept' = 'application/json'
            'openai-beta' = 'codex-1'
            'originator' = 'Codex Desktop'
        }
        if ($tokens.account_id) { $headers['ChatGPT-Account-Id'] = [string]$tokens.account_id }
        # This reads the signed-in account's reset inventory from the first-party host.
        $result = Invoke-QuotaRequest 'GET' 'https://chatgpt.com/backend-api/wham/rate-limit-reset-credits' $null $headers
        if ($null -ne $result)
        {
            $available = if ($null -ne $result.available_count) { $result.available_count } else { $result.availableCount }
            $credits = @($result.credits | ForEach-Object {
                $expiry = if ($_.expires_at) { $_.expires_at } else { $_.expiresAt }
                @{ status = $_.status; expires_at = $expiry }
            })
            return @{ available_count = $available; credits = $credits }
        }
    }
    catch {}
    return $null
}

$tools = $env:TOKEN_MONITOR_LITE_QUOTA_TOOLS -split ','
$antigravity = if ($tools -contains 'antigravity') { Get-AntigravityQuota }
$codexReset = if ($tools -contains 'codex') { Get-CodexBankedReset }
$deepseek = if ($tools -contains 'dsh')
{
    $deepseekKey = $env:DEEPSEEK_API_KEY
    if (!$deepseekKey) { $deepseekKey = $env:DEEPSEEK_KEY }
    if ($deepseekKey) { Invoke-QuotaRequest 'GET' 'https://api.deepseek.com/user/balance' $null @{ Authorization = "Bearer $($deepseekKey.Trim())" } }
}
ConvertTo-Json -Compress -Depth 20 -InputObject @{ antigravity = $antigravity; codex_reset = $codexReset; deepseek = $deepseek }
