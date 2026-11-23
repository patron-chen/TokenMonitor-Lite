function Invoke-LiteUpdateDownload
{
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $limit = [int]$env:LITE_UPDATE_TIMEOUT
    $status = 0
    try
    {
        $uri = [uri]$env:LITE_UPDATE_URL
        for ($redirects = 0; $redirects -le 10; $redirects++)
        {
            $left = $limit - $clock.ElapsedMilliseconds
            if ($left -le 0 -or !$uri.IsAbsoluteUri -or $uri.Scheme -ne 'https')
            {
                break
            }
            # Re-evaluate Lite's proxy rules for the GitHub asset host after a redirect.
            $reply = Invoke-LiteHttpRequest -Uri $uri -OutFile $env:LITE_UPDATE_FILE `
                -MaxBytes $env:LITE_UPDATE_MAX_BYTES -TimeoutMs $left
            if ($reply.Status -notin 301,302,303,307,308)
            {
                $status = $reply.Status
                break
            }
            if (!$reply.Location) { break }
            $uri = [uri]::new($uri, $reply.Location)
        }
    }
    catch {}
    [Console]::WriteLine($status)
}

Invoke-LiteUpdateDownload
