$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

function Read-Flag([string]$Command, [string]$Name)
{
    $match = [regex]::Match($Command, '(?i)(?:^|\s)--' + $Name + '[=\s]+(?:"([^"]+)"|([^\s"]+))')
    if ($match.Groups[1].Success) { return $match.Groups[1].Value }
    return $match.Groups[2].Value
}

function Read-Port([string]$Command, [string]$Name)
{
    $port = 0
    if ([int]::TryParse((Read-Flag $Command $Name), [ref]$port) -and $port -ge 1 -and $port -le 65535) { return $port }
    return 0
}

function Get-AntigravityEndpoints
{
    $filter = "Name LIKE 'language[_]server%' OR Name LIKE 'language-server%' OR Name LIKE 'agy%' OR Name LIKE 'antigravity-cli%'"
    $services = @(foreach ($process in (Get-CimInstance Win32_Process -Filter $filter))
    {
        $line = [string]$process.CommandLine
        $appDir = Read-Flag $line 'app_data_dir'
        $server = $process.Name -match '^language[_-]server'
        $app = $server -and ($appDir -in @('antigravity', 'antigravity-ide') -or $line -match '[/\\]antigravity[/\\]')
        $hub = $line -match '(?:^|\s)--hub(?:[=\s]|$)'
        $cli = $line -match '[/\\]antigravity[-_]cli[/\\]' -or
            ($process.Name -match '^(?:agy|antigravity-cli)(?:\.exe)?$' -and ($hub -or $line -match '\slanguage[_-]server(?:\s|$)'))
        if (!$app -and !$cli) { continue }
        $csrf = Read-Flag $line 'csrf_token'
        if (!$csrf -and ($app -or $hub)) { continue }
        $ide = $appDir -eq 'antigravity-ide' -or $line -match '[/\\]extensions[/\\]antigravity[/\\]bin[/\\]language[_-]server'
        [pscustomobject]@{
            rank = if ($app -and $ide) { 2 } elseif ($app) { 0 } else { 1 }
            pid = $process.ProcessId
            csrf = $csrf
            hub = Read-Port $line 'hub-port'
            extension = Read-Port $line 'extension_server_port'
            extensionCsrf = Read-Flag $line 'extension_server_csrf_token'
        }
    })
    if (!$services) { return }

    # Explicit and extension ports can be served by a different PID.
    try { $listeners = @(Get-NetTCPConnection -State Listen -OwningProcess $services.pid -ErrorAction Stop) }
    catch
    {
        # Some Windows installations cannot query NetTCPIP. Keep Lite's IPv4 listener fallback.
        $listeners = @(& "$env:SystemRoot\System32\netstat.exe" -ano -p TCP | ForEach-Object {
            if ($_ -match '^\s*TCP\s+(?:127\.0\.0\.1|0\.0\.0\.0):(\d+)\s+\S+\s+LISTENING\s+(\d+)\s*$')
            {
                [pscustomobject]@{ LocalPort = [int]$Matches[1]; OwningProcess = [int]$Matches[2] }
            }
        })
    }
    foreach ($service in ($services | Sort-Object rank, pid))
    {
        $ports = @($service.hub) + @($listeners | Where-Object OwningProcess -eq $service.pid |
            Select-Object -ExpandProperty LocalPort | Sort-Object -Unique)
        foreach ($port in ($ports | Where-Object { $_ -gt 0 } | Select-Object -Unique))
        {
            foreach ($secure in @($true, $false))
            {
                @{ rank = $service.rank; secure = $secure; port = $port; csrf = $service.csrf }
            }
        }
        if ($service.extension)
        {
            $token = if ($service.extensionCsrf) { $service.extensionCsrf } else { $service.csrf }
            if ($service.extension -notin $ports -or $token -cne $service.csrf)
            {
                @{ rank = $service.rank; secure = $false; port = $service.extension; csrf = $token }
            }
        }
    }
}

[Console]::WriteLine((ConvertTo-Json -Compress -InputObject @(Get-AntigravityEndpoints)))
