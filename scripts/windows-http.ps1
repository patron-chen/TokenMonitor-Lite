# Shared by the quota reader, updater and build-time scanner download.
function Test-LiteProxyNetwork([Net.IPAddress]$Address, [string]$Rule)
{
    if (!$Address) { return $false }
    $networkText, $prefixText = $Rule -split '/', 2
    $network = $null
    $prefix = 0
    if (![Net.IPAddress]::TryParse($networkText, [ref]$network) -or ![int]::TryParse($prefixText, [ref]$prefix) -or
        $Address.AddressFamily -ne $network.AddressFamily)
    {
        return $false
    }

    $actual = $Address.GetAddressBytes()
    $expected = $network.GetAddressBytes()
    if ($prefix -lt 0 -or $prefix -gt $actual.Length * 8) { return $false }
    # Compare the high bits of the last byte when the prefix falls between byte boundaries.
    for ($i = 0; $prefix -gt 0; $i++)
    {
        $shift = 8 - [Math]::Min($prefix, 8)
        if (($actual[$i] -shr $shift) -ne ($expected[$i] -shr $shift)) { return $false }
        $prefix -= 8
    }
    return $true
}

function Test-LiteAppProxyBypass([uri]$Uri, [string]$Rules)
{
    $hostName = $Uri.IdnHost.TrimEnd('.').Trim('[', ']')
    if ($hostName -in 'localhost','loopback' -or $hostName.EndsWith('.localhost', [StringComparison]::OrdinalIgnoreCase))
    {
        return $true
    }

    # Match local literals without DNS, as the original app does.
    $address = $null
    if ([Net.IPAddress]::TryParse($hostName, [ref]$address))
    {
        $local = if ($address.IsIPv4MappedToIPv6) { $address.MapToIPv4() } else { $address }
        $octets = $local.GetAddressBytes()
        if ([Net.IPAddress]::IsLoopback($local) -or $local.IsIPv6LinkLocal -or
            ($octets.Length -eq 4 -and $octets[0] -eq 169 -and $octets[1] -eq 254))
        {
            return $true
        }
    }

    foreach ($rule in $Rules -split ',')
    {
        if ($rule -eq '*') { return $true }
        if ($rule.Contains('/'))
        {
            if (Test-LiteProxyNetwork $address $rule) { return $true }
            continue
        }
        if ($rule -notmatch '^(?<host>\[[^\]]+\]|[^:]+)(?::(?<port>[0-9]+))?$') { continue }
        if ($Matches.port -and [int]$Matches.port -ne $Uri.Port) { continue }
        $name = $Matches.host.Trim('[', ']')
        $entry = $null
        if ([Net.IPAddress]::TryParse($name, [ref]$entry))
        {
            if ($address -and $address.Equals($entry)) { return $true }
        }
        elseif (!$address -and ($hostName -eq $name -or $hostName.EndsWith('.' + $name, [StringComparison]::OrdinalIgnoreCase)))
        {
            return $true
        }
    }
    return $false
}

function Get-LiteProxy([uri]$Uri, [hashtable]$Decision = @{})
{
    $Decision.Source = 'loopback'
    if ($Uri.IsLoopback) { return 'DIRECT' }
    $appProxy = $env:TOKEN_MONITOR_LITE_APP_PROXY_URL
    if ($appProxy)
    {
        if (Test-LiteAppProxyBypass $Uri $env:TOKEN_MONITOR_LITE_APP_NO_PROXY)
        {
            $Decision.Source = 'app_bypass'
            return 'DIRECT'
        }
        $Decision.Source = 'app_settings'
        return $appProxy
    }

    $Decision.Source = 'NO_PROXY'
    $hostName = $Uri.DnsSafeHost.TrimEnd('.')
    foreach ($entry in ([string]$env:NO_PROXY -split ','))
    {
        $rule = $entry.Trim()
        if ($rule -eq '*') { return 'DIRECT' }
        $rule = $rule -replace (':' + $Uri.Port + '$'), ''
        $rule = ($rule -replace '^\*\.', '').Trim('.')
        if ($rule -and ($hostName -eq $rule -or $hostName.EndsWith('.' + $rule, [StringComparison]::OrdinalIgnoreCase)))
        {
            return 'DIRECT'
        }
    }

    $Decision.Source = 'proxy-server'
    $proxy = $env:TOKEN_MONITOR_LITE_PROXY_SERVER
    if ([string]::IsNullOrWhiteSpace($proxy))
    {
        $Decision.Source = $Uri.Scheme.ToUpperInvariant() + '_PROXY'
        $proxy = [Environment]::GetEnvironmentVariable($Decision.Source)
    }
    if ([string]::IsNullOrWhiteSpace($proxy))
    {
        $Decision.Source = 'ALL_PROXY'
        $proxy = $env:ALL_PROXY
    }
    if (![string]::IsNullOrWhiteSpace($proxy))
    {
        $proxy = $proxy.Trim()
        if ($proxy -notmatch '^[a-z][a-z0-9+.-]*://') { $proxy = 'http://' + $proxy }
        return $proxy
    }

    # WinHttpRequest's default reads the machine WinHTTP settings only. Resolve user settings/PAC first.
    try
    {
        $systemProxy = [Net.WebRequest]::GetSystemWebProxy().GetProxy($Uri)
        if ($systemProxy -and $systemProxy -ne $Uri)
        {
            $Decision.Source = 'windows_user'
            return $systemProxy.AbsoluteUri
        }
    }
    catch {} # WinHTTP can still have a machine proxy if the user settings are unavailable.
    $Decision.Source = 'winhttp_default'
    return $null
}

function ConvertTo-CurlConfigValue([string]$Value)
{
    return '"' + $Value.Replace('\', '\\').Replace('"', '\"').Replace([string][char]13, '\r').Replace([string][char]10, '\n') + '"'
}

function Invoke-LiteCurlRequest([uri]$Uri, $Headers, [string]$Proxy, [string]$OutFile, [int]$MaxBytes, [int]$TimeoutMs, [string]$Method, [string]$Body, [scriptblock]$Trace)
{
    $curl = Join-Path ([Environment]::SystemDirectory) 'curl.exe'
    # Headers and JSON bodies travel on stdin so credentials stay out of the process command line.
    $config = @(foreach ($header in $Headers.GetEnumerator())
    {
        'header = ' + (ConvertTo-CurlConfigValue ('{0}: {1}' -f $header.Key, $header.Value))
    })
    if ($Method -eq 'POST') { $config += 'data-raw = ' + (ConvertTo-CurlConfigValue $Body) }
    $configBytes = [Text.Encoding]::UTF8.GetBytes(($config -join [Environment]::NewLine) + [Environment]::NewLine)
    # .NET Framework prepends the console encoding's BOM to redirected stdin, corrupting the first header.
    if ([Console]::InputEncoding.CodePage -eq 65001) { [Console]::InputEncoding = [Text.UTF8Encoding]::new($false) }
    $start = [Diagnostics.ProcessStartInfo]::new($curl)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.StandardOutputEncoding = [Text.Encoding]::UTF8
    # Get-LiteProxy already checked bypass rules before selecting this proxy.
    $start.EnvironmentVariables.Remove('NO_PROXY')
    $options = '-q --silent --config - --request {0} --write-out "\n%{{http_code}} %{{redirect_url}}"' -f $Method
    $connectTimeout = [int][Math]::Ceiling($TimeoutMs / 1000.0)
    if ($OutFile)
    {
        $options += ' --output "{0}" --max-filesize {1}' -f $OutFile, $MaxBytes
        $connectTimeout = 8
    }

    $routes = @($Proxy)
    if ($Proxy -match '^socks5://')
    {
        # Local DNS can fail behind SOCKS. Retry through the same proxy using its resolver.
        $routes += $Proxy -replace '^socks5:', 'socks5h:'
    }
    $variable = $Uri.Scheme + '_PROXY'
    $elapsed = [Diagnostics.Stopwatch]::StartNew()
    foreach ($route in $routes)
    {
        $left = $TimeoutMs - $elapsed.ElapsedMilliseconds
        if ($left -le 0) { break }
        $seconds = ($left / 1000).ToString('0.###', [Globalization.CultureInfo]::InvariantCulture)
        $start.EnvironmentVariables[$variable] = $route
        # Reserve time for the proxy DNS retry if local SOCKS5 name resolution stalls.
        $connect = if ($route -match '^socks5://') { [Math]::Min(2, $connectTimeout) } else { $connectTimeout }
        $start.Arguments = $options + (' --connect-timeout {0} --max-time {1} -- "{2}"' -f $connect, $seconds, $Uri.AbsoluteUri)
        $process = [Diagnostics.Process]::Start($start)
        try
        {
            # Send headers through a dedicated pipe, including when the tray helper itself has no stdin.
            $inputStream = $process.StandardInput.BaseStream
            $inputStream.Write($configBytes, 0, $configBytes.Length)
            $inputStream.Close()
            $reply = $process.StandardOutput.ReadToEnd()
            $process.WaitForExit()
            $exitCode = $process.ExitCode
        }
        finally { $process.Dispose() }
        $marker = $reply.LastIndexOf([char]10)
        $status, $location = $reply.Substring($marker + 1) -split ' ', 2
        if ($Trace)
        {
            $code = if ($marker -ge 0 -and $status -match '^[0-9]{3}$') { [int]$status } else { 0 }
            $proxyDns = $route -match '^socks(4a|5h)://'
            & $Trace "curl exit=$exitCode http=$code socks_proxy_dns=$proxyDns elapsed_ms=$($elapsed.ElapsedMilliseconds)"
        }
        if ($exitCode -eq 0 -and $marker -ge 0)
        {
            $body = if (!$OutFile -and $status -eq '200') { $reply.Substring(0, $marker) }
            return @{ Status = [int]$status; Location = $location; Text = $body }
        }
        if ($status -ne '000') { break }
    }
    throw 'Connection through the configured proxy failed.'
}

function Invoke-LiteHttpRequest(
    [uri]$Uri, $Headers = @{}, [string]$OutFile, [int]$MaxBytes,
    [ValidateSet('GET', 'POST')][string]$Method = 'GET', [string]$Body, [int]$TimeoutMs = 5000, [switch]$LocalServiceCertificate,
    [scriptblock]$Trace)
{
    $ErrorActionPreference = 'Stop'
    if ($LocalServiceCertificate -and !$Uri.IsLoopback) { throw 'Local service certificates require a loopback address.' }
    foreach ($header in $Headers.GetEnumerator())
    {
        if ($header.Key -match '[\r\n]' -or $header.Value -match '[\r\n]') { throw 'Invalid HTTP header.' }
    }
    $decision = @{}
    if ($Trace) { & $Trace 'route resolving' }
    $route = Get-LiteProxy $Uri $decision
    if ($Trace) { & $Trace "route source=$($decision.Source)" }
    if ($route -and $route -ne 'DIRECT')
    {
        $proxy = [uri]$route
        $useCurl = $proxy.Scheme -ne 'http' -or $proxy.UserInfo
        if ($Trace)
        {
            # Only log an allowlisted scheme, locality and port. A proxy URL can contain credentials.
            $scheme = if ($proxy.Scheme -in 'http','https','socks4','socks4a','socks5','socks5h') { $proxy.Scheme } else { 'other' }
            $client = if ($useCurl) { 'curl' } else { 'winhttp' }
            & $Trace "transport client=$client proxy_scheme=$scheme proxy_loopback=$($proxy.IsLoopback) port=$($proxy.Port)"
        }
        if ($useCurl)
        {
            return Invoke-LiteCurlRequest $Uri $Headers $route $OutFile $MaxBytes $TimeoutMs $Method $Body $Trace
        }
    }
    elseif ($Trace)
    {
        $mode = if ($route -eq 'DIRECT') { 'direct' } else { 'system_default' }
        & $Trace "transport client=winhttp mode=$mode"
    }
    $external = !$Uri.IsLoopback
    $elapsed = [Diagnostics.Stopwatch]::StartNew()
    $request = New-Object -ComObject WinHttp.WinHttpRequest.5.1
    if ($OutFile) { $request.SetTimeouts([Math]::Min(4000,$TimeoutMs), [Math]::Min(8000,$TimeoutMs), [Math]::Min(8000,$TimeoutMs), $TimeoutMs) }
    elseif ($external) { $request.SetTimeouts($TimeoutMs, $TimeoutMs, $TimeoutMs, $TimeoutMs) }
    else { $request.SetTimeouts([Math]::Min(800,$TimeoutMs), [Math]::Min(800,$TimeoutMs), [Math]::Min(1000,$TimeoutMs), [Math]::Min(2600,$TimeoutMs)) }
    $request.Open($Method, $Uri.AbsoluteUri, $external)
    if ($route -eq 'DIRECT') { $request.SetProxy(1) }
    elseif ($route) { $request.SetProxy(2, $proxy.Authority) }
    $request.Option(6) = $false
    # Antigravity presents its own certificate on its discovered loopback port.
    if ($LocalServiceCertificate -and $Uri.Scheme -eq 'https') { $request.Option(4) = 0x1100 }
    $request.SetAutoLogonPolicy(2)
    foreach ($header in $Headers.GetEnumerator())
    {
        $request.SetRequestHeader($header.Key, $header.Value)
    }
    if ($Trace) { & $Trace "winhttp phase=send timeout_ms=$TimeoutMs" }
    if ($Method -eq 'POST') { $request.Send([Text.Encoding]::UTF8.GetBytes($Body)) }
    else { $request.Send() }
    if ($external)
    {
        # WinHTTP's socket timeouts apply per operation. Bound the entire asynchronous exchange as well.
        $left = $TimeoutMs - $elapsed.ElapsedMilliseconds
        if ($Trace) { & $Trace "winhttp phase=wait elapsed_ms=$($elapsed.ElapsedMilliseconds)" }
        if ($left -le 0 -or !$request.WaitForResponse([int][Math]::Ceiling($left / 1000.0)))
        {
            $request.Abort()
            if ($Trace) { & $Trace "winhttp result=timeout elapsed_ms=$($elapsed.ElapsedMilliseconds)" }
            throw [TimeoutException]::new('HTTP request timed out.')
        }
    }
    $result = @{ Status = [int]$request.Status }
    if ($result.Status -eq 200)
    {
        if ($OutFile)
        {
            [byte[]]$bytes = $request.ResponseBody
            if ($bytes.Length -gt $MaxBytes) { throw 'Download exceeds the size limit.' }
            [IO.File]::WriteAllBytes($OutFile, $bytes)
        }
        else { $result.Text = $request.ResponseText }
    }
    elseif ($OutFile -and $result.Status -in 301,302,303,307,308)
    {
        $result.Location = $request.GetResponseHeader('Location')
    }
    return $result
}

# Keep one worker for a sequence of short RPCs. Each input line uses the same parameters as an external request.
function Read-LiteHttpRequests
{
    [Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    while ($null -ne ($line = [Console]::ReadLine()))
    {
        $body = $null
        try
        {
            $message = ConvertFrom-Json -InputObject $line
            $parameters = @{}
            foreach ($property in $message.PSObject.Properties) { $parameters[$property.Name] = $property.Value }
            $headers = @{}
            foreach ($property in $message.Headers.PSObject.Properties) { $headers[$property.Name] = $property.Value }
            $parameters.Headers = $headers
            $reply = Invoke-LiteHttpRequest @parameters
            if ($reply.Status -eq 200) { $body = $reply.Text }
        }
        catch {}
        if ($null -eq $body) { [Console]::WriteLine('null') }
        else { [Console]::WriteLine((ConvertTo-Json -Compress -InputObject $body)) }
    }
}
