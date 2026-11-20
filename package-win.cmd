@echo off
setlocal
pushd "%~dp0" || exit /b 1

for %%T in (node.exe npm.cmd cargo.exe tar.exe) do (
    where.exe %%T >nul 2>&1
    if errorlevel 1 (
        echo [ERROR] %%T is required on PATH. See README.md.
        goto :failed
    )
)

echo [1/2] Preparing frontend dependencies...
rem Use npm ci on a fresh checkout and reuse the installed tree on later runs.
if exist "node_modules\." (
    call npm.cmd install --include=dev --prefer-offline --engine-strict --no-audit --no-fund
) else (
    call npm.cmd ci --include=dev --engine-strict --no-audit --no-fund
)
if errorlevel 1 goto :failed

echo [2/2] Building the Windows x64 portable ZIP...
call npm.cmd run dist:win
if errorlevel 1 goto :failed
echo [OK] ZIP saved under "%CD%\release".
set "package_exit=0"
goto :finish

:failed
echo [ERROR] Packaging failed. See the output above.
set "package_exit=1"

:finish
popd
pause
exit /b %package_exit%
