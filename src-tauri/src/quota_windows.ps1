$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

function Get-QuotaResponse($Uri, $Headers)
{
    try
    {
        $request = New-Object -ComObject WinHttp.WinHttpRequest.5.1
        $request.SetTimeouts(800, 800, 1000, 2600)
        $request.Open('GET', $Uri, $false)
        $request.Option(6) = $false
        $request.SetAutoLogonPolicy(2)
        foreach ($header in $Headers.GetEnumerator())
        {
            $request.SetRequestHeader($header.Key, $header.Value)
        }
        $request.Send()
        if ($request.Status -eq 200) { return ConvertFrom-Json -InputObject $request.ResponseText }
    }
    catch {}
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
        $result = Get-QuotaResponse 'https://chatgpt.com/backend-api/wham/rate-limit-reset-credits' $headers
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
$codexReset = if ($tools -contains 'codex') { Get-CodexBankedReset }
$deepseek = if ($tools -contains 'dsh')
{
    $deepseekKey = $env:DEEPSEEK_API_KEY
    if (!$deepseekKey) { $deepseekKey = $env:DEEPSEEK_KEY }
    if ($deepseekKey) { Get-QuotaResponse 'https://api.deepseek.com/user/balance' @{ Authorization = "Bearer $($deepseekKey.Trim())" } }
}
ConvertTo-Json -Compress -Depth 20 -InputObject @{ codex_reset = $codexReset; deepseek = $deepseek }
