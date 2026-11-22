@echo off
rem Usage: build-setup.bat <minima.exe> <version> <arch> <dist-dir>
rem Builds the graphical Inno Setup installer. Skips with a message if Inno Setup is missing.
setlocal
set "BIN=%~1"
set "VER=%~2"
set "ARCH=%~3"
set "OUT=%~4"
set "ISCC="
if exist "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" set "ISCC=%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
if not defined ISCC if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if not defined ISCC if exist "%ProgramFiles%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles%\Inno Setup 6\ISCC.exe"
if not defined ISCC for /f "delims=" %%I in ('where iscc 2^>nul') do set "ISCC=%%I"
if not defined ISCC goto missing
set "HERE=%~dp0"
set "REPO=%HERE%..\..\.."
pushd "%REPO%" >nul
set "REPO=%CD%"
popd >nul
"%ISCC%" /Q "/DAppVersion=%VER%" "/DSourceExe=%BIN%" "/DArch=%ARCH%" "/DOutDir=%OUT%" "/DRepoRoot=%REPO%" "%HERE%minima.iss"
if errorlevel 1 goto failed
echo   wrote %OUT%\minima-%VER%-windows-%ARCH%-setup.exe
goto done
:missing
echo     (graphical installer skipped: Inno Setup 6 not found - run: winget install JRSoftware.InnoSetup)
goto done
:failed
echo !! Inno Setup failed
endlocal
exit /b 1
:done
endlocal
