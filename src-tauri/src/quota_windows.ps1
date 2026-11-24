$ErrorActionPreference = 'Stop'

function Write-QuotaTrace([string]$Source, [string]$Message)
{
    try
    {
        # Stream progress before returning the quota JSON. Lite owns the log file and its rotation.
        [Console]::Error.WriteLine("lite-log $Source $Message")
    }
    catch {} # Diagnostics are best effort and must never enter the helper's JSON output.
}

function Write-CodexQuotaTrace([string]$Message) { Write-QuotaTrace "codex_quota trigger=$env:TOKEN_MONITOR_LITE_QUOTA_TRIGGER" $Message }
function Write-CodexUsageTrace([string]$Message) { Write-QuotaTrace "codex_usage trigger=$env:TOKEN_MONITOR_LITE_QUOTA_TRIGGER" $Message }
function Write-BankedResetTrace([string]$Message) { Write-QuotaTrace "banked_reset trigger=$env:TOKEN_MONITOR_LITE_QUOTA_TRIGGER" $Message }

function Get-QuotaResponse([uri]$Uri, $Headers, [scriptblock]$Trace)
{
    $stage = 'request'
    $elapsed = [Diagnostics.Stopwatch]::StartNew()
    try
    {
        # Remote quota reads may need a proxy tunnel and a TLS handshake before the first response byte.
        $response = Invoke-LiteHttpRequest -Uri $Uri -Headers $Headers -TimeoutMs 20000 -Trace $Trace
        if ($Trace) { & $Trace "http status=$([int]$response.Status) elapsed_ms=$($elapsed.ElapsedMilliseconds)" }
        if ($response.Status -eq 200)
        {
            $stage = 'response_json'
            if ([string]::IsNullOrWhiteSpace($response.Text))
            {
                if ($Trace) { & $Trace 'response result=empty_body' }
                return $null
            }
            $result = ConvertFrom-Json -InputObject $response.Text
            if ($Trace -and $null -eq $result) { & $Trace 'response result=null' }
            return $result
        }
    }
    catch
    {
        if ($Trace)
        {
            $code = '{0:X8}' -f $_.Exception.GetBaseException().HResult
            & $Trace "failure stage=$stage hresult=0x$code elapsed_ms=$($elapsed.ElapsedMilliseconds)"
        }
    }
    return $null
}

function Get-CodexHeaders
{
    $trace = ${function:Write-CodexQuotaTrace}
    $stage = 'auth_read'
    try
    {
        & $Trace "auth lookup codex_home_set=$(![string]::IsNullOrWhiteSpace($env:CODEX_HOME))"
        if (!$env:TOKEN_MONITOR_LITE_CODEX_AUTH) { & $Trace 'auth result=no_path'; return $null }
        if (!(Test-Path -LiteralPath $env:TOKEN_MONITOR_LITE_CODEX_AUTH -PathType Leaf))
        {
            & $Trace 'auth result=file_missing'
            return $null
        }
        $contents = Get-Content -Raw -LiteralPath $env:TOKEN_MONITOR_LITE_CODEX_AUTH
        $stage = 'auth_json'
        if ([string]::IsNullOrWhiteSpace($contents)) { & $Trace 'auth result=empty_file'; return $null }
        $auth = ConvertFrom-Json -InputObject $contents
        $tokens = if ($auth.tokens) { $auth.tokens } else { $auth }
        $access = ([string]$tokens.access_token).Trim()
        if (!$access)
        {
            & $Trace "auth result=no_access_token api_key_present=$([bool]$auth.OPENAI_API_KEY)"
            return $null
        }
        $headers = @{
            'Authorization' = "Bearer $access"
            'Accept' = 'application/json'
            'openai-beta' = 'codex-1'
            'originator' = 'Codex Desktop'
        }
        if ($tokens.account_id) { $headers['ChatGPT-Account-Id'] = [string]$tokens.account_id }
        & $Trace "auth result=ready account_header=$($headers.ContainsKey('ChatGPT-Account-Id'))"
        return $headers
    }
    catch
    {
        $code = '{0:X8}' -f $_.Exception.GetBaseException().HResult
        & $Trace "failure stage=$stage hresult=0x$code"
    }
    return $null
}

function Get-CodexUsage($Headers)
{
    $trace = ${function:Write-CodexUsageTrace}
    $usage = Get-QuotaResponse 'https://chatgpt.com/backend-api/wham/usage' $Headers $trace
    if ($null -eq $usage) { return @{ error = 'no_usage_response' } }
    $account = if ($usage.account_id) { $usage.account_id } else { $Headers['ChatGPT-Account-Id'] }
    # The numeric reading keeps its own timestamp while Banked Reset may still be in flight.
    return @{ rate_limit = $usage.rate_limit; plan_type = $usage.plan_type; account_id = $account
        observed_at = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
}

function Get-CodexBankedReset($Headers)
{
    try
    {
        # This reads the signed-in account's reset inventory from the first-party host.
        $result = Get-QuotaResponse 'https://chatgpt.com/backend-api/wham/rate-limit-reset-credits' $Headers ${function:Write-BankedResetTrace}
        if ($null -ne $result)
        {
            $available = if ($null -ne $result.available_count) { $result.available_count } else { $result.availableCount }
            $field = if ($null -ne $result.available_count) { 'available_count' } elseif ($null -ne $result.availableCount) { 'availableCount' }
                elseif ($result.PSObject.Properties['available_count'] -or $result.PSObject.Properties['availableCount']) { 'null' } else { 'missing' }
            $numeric = [uint64]0
            $valid = ($available -is [string] -or $available -is [int] -or $available -is [long] -or $available -is [uint64]) -and
                [uint64]::TryParse([string]$available, [Globalization.NumberStyles]::None, [Globalization.CultureInfo]::InvariantCulture, [ref]$numeric)
            if ($valid) { Write-BankedResetTrace "response count_field=$field count=$numeric" }
            else { Write-BankedResetTrace "response count_field=$field count=unusable" }
            $credits = @($result.credits | ForEach-Object {
                $expiry = if ($_.expires_at) { $_.expires_at } else { $_.expiresAt }
                @{ status = $_.status; expires_at = $expiry }
            })
            return @{ available_count = $available; credits = $credits }
        }
    }
    catch
    {
        $code = '{0:X8}' -f $_.Exception.GetBaseException().HResult
        Write-BankedResetTrace "failure stage=response_fields hresult=0x$code"
    }
    return $null
}

function Get-CodexQuota
{
    $headers = Get-CodexHeaders
    if ($null -eq $headers) { return @{ usage = @{ error = 'auth_unavailable' }; reset = $null } }

    # Run both HTTP requests in one helper with the same account and independent timeouts.
    $worker = [PowerShell]::Create()
    try
    {
        [void]$worker.AddScript($quotaFunctions.ToString(), $false).AddStatement().AddCommand('Get-CodexUsage').AddArgument($headers)
        $pending = $worker.BeginInvoke()
        $reset = Get-CodexBankedReset $headers
        try { $usage = $worker.EndInvoke($pending)[0] }
        catch { $usage = @{ error = 'usage_worker_failed' } }
        return @{ usage = $usage; reset = $reset }
    }
    finally { $worker.Dispose() }
}

function Read-LiteQuotas
{
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    $tools = $env:TOKEN_MONITOR_LITE_QUOTA_TOOLS -split ','
    $codex = if ($tools -contains 'codex') { Get-CodexQuota }
    $deepseek = if ($tools -contains 'dsh')
    {
        $deepseekKey = $env:DEEPSEEK_API_KEY
        if (!$deepseekKey) { $deepseekKey = $env:DEEPSEEK_KEY }
        if ($deepseekKey) { Get-QuotaResponse 'https://api.deepseek.com/user/balance' @{ Authorization = "Bearer $($deepseekKey.Trim())" } }
    }
    ConvertTo-Json -Compress -Depth 20 -InputObject @{ codex = $codex; deepseek = $deepseek }
}
