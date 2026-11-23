$ErrorActionPreference = 'Stop'
# Keep the executable last so a failed sidecar or documentation replacement leaves it in place.
$script:LitePackageFiles = @(
    'tokscale.exe', 'README.md', 'LICENSE-Token-Monitor.txt', 'LICENSE-Tokscale.txt',
    'LICENSE-LiteLLM.txt', 'ICON-NOTICES.md', 'TokenMonitorLite.exe'
)

function Open-LiteUpdateArchive([string]$Path, [string]$Sha256)
{
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stream = [IO.File]::OpenRead($Path)
    $archive = $null
    try
    {
        # Verify and read the same file stream without depending on PowerShell module discovery.
        $algorithm = [Security.Cryptography.SHA256]::Create()
        try { $hash = [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '') }
        finally { $algorithm.Dispose() }
        if ($hash -ne $Sha256) { throw 'Update archive checksum mismatch.' }
        $stream.Position = 0
        $archive = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Read)
        $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $total = 0L
        foreach ($entry in $archive.Entries)
        {
            $total += $entry.Length
            if ($entry.FullName -cnotin $script:LitePackageFiles -or !$names.Add($entry.FullName) -or
                $entry.Length -le 0 -or $total -gt 200MB)
            {
                throw 'Invalid portable update file list.'
            }
        }
        if ($names.Count -ne $script:LitePackageFiles.Count)
        {
            throw 'Incomplete portable update.'
        }
        return $archive
    }
    catch
    {
        if ($archive) { $archive.Dispose() } else { $stream.Dispose() }
        throw
    }
}

function Test-LiteUpdateArchive
{
    try
    {
        $archive = Open-LiteUpdateArchive $env:LITE_UPDATE_FILE $env:LITE_UPDATE_SHA256
        $archive.Dispose()
        [Console]::WriteLine('ok')
    }
    catch { [Console]::WriteLine('invalidArchive') }
}

function Clear-LiteUpdateStage([string]$Stage)
{
    if (!$Stage) { return }
    foreach ($name in $script:LitePackageFiles)
    {
        try { [IO.File]::Delete([IO.Path]::Combine($Stage, $name)) } catch {}
        try { [IO.File]::Delete([IO.Path]::Combine($Stage, $name + '.bak')) } catch {}
    }
    try { [IO.Directory]::Delete($Stage) } catch {}
}

function Start-LiteUpdatedApp([string]$Path)
{
    $start = [Diagnostics.ProcessStartInfo]::new($Path)
    $start.UseShellExecute = $false
    $start.WorkingDirectory = [IO.Path]::GetDirectoryName($Path)
    # Lite already normalized --proxy-server into its inherited environment.
    foreach ($name in 'FILE','SHA256','EXE','PARENT','RESULT')
    {
        $start.EnvironmentVariables.Remove('LITE_UPDATE_' + $name)
    }
    $process = [Diagnostics.Process]::Start($start)
    $process.Dispose()
}

function Save-LiteUpdateResult([string]$Result)
{
    try { [IO.File]::WriteAllText($env:LITE_UPDATE_RESULT, $Result) } catch {}
}

function Install-LiteUpdate
{
    $stage = $null
    $parent = $null
    $failure = 'installDirectory'
    try
    {
        $app = [IO.Path]::GetFullPath($env:LITE_UPDATE_EXE)
        $directory = [IO.Path]::GetDirectoryName($app)
        $failure = 'installFailed'
        $parent = [Diagnostics.Process]::GetProcessById([int]$env:LITE_UPDATE_PARENT)
        # Acquire the process handle before asking the tray to exit.
        $null = $parent.Handle
        $failure = 'invalidArchive'
        $archive = Open-LiteUpdateArchive $env:LITE_UPDATE_FILE $env:LITE_UPDATE_SHA256
        try
        {
            $failure = 'installDirectory'
            $stage = [IO.Path]::Combine($directory, '.lite-update-' + [guid]::NewGuid().ToString('N'))
            $null = [IO.Directory]::CreateDirectory($stage)
            $files = @(foreach ($name in $script:LitePackageFiles)
            {
                $target = if ($name -ceq 'TokenMonitorLite.exe') { $app } else { [IO.Path]::Combine($directory, $name) }
                if ([IO.Directory]::Exists($target) -or ([IO.File]::Exists($target) -and
                    ([IO.File]::GetAttributes($target) -band [IO.FileAttributes]::ReadOnly)))
                {
                    throw 'Portable update target is read-only.'
                }
                $next = [IO.Path]::Combine($stage, $name)
                [IO.Compression.ZipFileExtensions]::ExtractToFile($archive.GetEntry($name), $next)
                [pscustomobject]@{ Target = $target; Next = $next; Backup = $next + '.bak'; Saved = $false; Replaced = $false }
            })
        }
        finally { $archive.Dispose() }
    }
    catch
    {
        Clear-LiteUpdateStage $stage
        if ($parent) { $parent.Dispose() }
        [Console]::WriteLine($failure)
        return
    }

    [Console]::WriteLine('ok')
    [Console]::Out.Flush()
    $result = 'installFailed'
    try
    {
        if (!$parent.WaitForExit(60000))
        {
            Save-LiteUpdateResult $result
            return
        }
        foreach ($file in $files)
        {
            if ([IO.File]::Exists($file.Target))
            {
                [IO.File]::Move($file.Target, $file.Backup)
                $file.Saved = $true
            }
            [IO.File]::Move($file.Next, $file.Target)
            $file.Replaced = $true
        }
        Save-LiteUpdateResult 'updated'
        Start-LiteUpdatedApp $app
        $result = 'updated'
    }
    catch
    {
        for ($i = $files.Count - 1; $i -ge 0; $i--)
        {
            $file = $files[$i]
            try
            {
                if ($file.Replaced) { [IO.File]::Delete($file.Target) }
                if ($file.Saved) { [IO.File]::Move($file.Backup, $file.Target) }
            }
            catch { $result = 'rollbackFailed' }
        }
        Save-LiteUpdateResult $result
        if ($parent.HasExited)
        {
            try { Start-LiteUpdatedApp $app } catch {}
        }
    }
    finally
    {
        $parent.Dispose()
        if ($result -ne 'rollbackFailed') { Clear-LiteUpdateStage $stage }
    }
}
