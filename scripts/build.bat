@echo off
rem Build Minima and package distributables into .\dist  (Windows)
rem
rem   scripts\build.bat                  release build for this machine + installers
rem   scripts\build.bat /target TRIPLE   cross-build (e.g. aarch64-pc-windows-msvc), repeatable
rem   scripts\build.bat /all             every target in scripts\targets.txt that can be built here
rem   scripts\build.bat /noinstallers    only the binary + portable zip
rem   scripts\build.bat /debug | /clean | /help
rem
rem Produces in dist\:
rem   minima-VER-windows-ARCH.zip                portable archive
rem   minima-VER-windows-ARCH-installer-cli.exe  minimal CLI installer (no GUI; works in any console)
rem   minima-VER-windows-ARCH-setup.exe          graphical installer (needs Inno Setup 6: winget install JRSoftware.InnoSetup)
setlocal EnableExtensions EnableDelayedExpansion

set "ROOT=%~dp0.."
pushd "%ROOT%" >nul
set "ROOT=%CD%"
set "DIST=%ROOT%\dist"
set "PROFILE=release"
set "PFLAG=--release"
set "INSTALLERS=1"
set "ALL=0"
set "TARGETS="

:parse
if "%~1"=="" goto parsed
if /I "%~1"=="/target"       ( set "TARGETS=!TARGETS! %~2" & shift & shift & goto parse )
if /I "%~1"=="--target"      ( set "TARGETS=!TARGETS! %~2" & shift & shift & goto parse )
if /I "%~1"=="/all"          ( set "ALL=1" & shift & goto parse )
if /I "%~1"=="--all"         ( set "ALL=1" & shift & goto parse )
if /I "%~1"=="/noinstallers" ( set "INSTALLERS=0" & shift & goto parse )
if /I "%~1"=="--no-installers" ( set "INSTALLERS=0" & shift & goto parse )
if /I "%~1"=="/debug"        ( set "PROFILE=debug" & set "PFLAG=" & shift & goto parse )
if /I "%~1"=="--debug"       ( set "PROFILE=debug" & set "PFLAG=" & shift & goto parse )
if /I "%~1"=="/clean"        ( rmdir /s /q "%DIST%" 2>nul & rmdir /s /q "%ROOT%\target" 2>nul & echo cleaned & goto end )
if /I "%~1"=="--clean"       ( rmdir /s /q "%DIST%" 2>nul & rmdir /s /q "%ROOT%\target" 2>nul & echo cleaned & goto end )
if /I "%~1"=="/help"         goto usage
if /I "%~1"=="--help"        goto usage
if /I "%~1"=="/?"            goto usage
echo unknown option: %~1
goto usage

:parsed
where cargo >nul 2>nul || ( echo error: Rust is not installed. Get it from https://rustup.rs & exit /b 1 )

for /f "tokens=2 delims== " %%v in ('findstr /B /C:"version = " "%ROOT%\Cargo.toml"') do ( set "VERSION=%%~v" & goto gotver )
:gotver
for /f "tokens=2" %%h in ('rustc -vV ^| findstr /B "host:"') do set "HOST=%%h"

if "%ALL%"=="1" (
  for /f "usebackq eol=# tokens=*" %%t in ("%ROOT%\scripts\targets.txt") do set "TARGETS=!TARGETS! %%t"
)
if "%TARGETS%"=="" set "TARGETS=%HOST%"

echo =^> Minima %VERSION%  (host: %HOST%)
if not exist "%DIST%" mkdir "%DIST%"

cargo build --release -q -p xtask || goto fail
set "XTASK=%ROOT%\target\release\xtask.exe"

set "BUILT=0"
for %%T in (%TARGETS%) do (
  call :build_one %%T
)

"%XTASK%" sums "%DIST%" >nul
echo.
echo =^> done: %BUILT% target(s) built. Contents of dist\:
dir /b "%DIST%"
goto end

:build_one
set "T=%~1"
set "ARCH=x86_64"
echo %T% | findstr /B /C:"aarch64" >nul && set "ARCH=aarch64"
echo %T% | findstr /B /C:"i686" >nul && set "ARCH=i686"
set "OS=windows"
echo %T% | findstr /C:"linux" >nul && set "OS=linux"
echo %T% | findstr /C:"apple" >nul && set "OS=macos"
set "EXT="
if "%OS%"=="windows" set "EXT=.exe"
echo.
echo =^> [%T%] building (%OS%/%ARCH%)
if /I not "%T%"=="%HOST%" rustup target add %T% >nul 2>nul
set "BUILD=cargo build %PFLAG% --target %T%"
if /I not "%T%"=="%HOST%" ( where cargo-zigbuild >nul 2>nul && set "BUILD=cargo zigbuild %PFLAG% --target %T%" )
%BUILD% -p minima
if errorlevel 1 ( echo !! skipped %T%: build failed - missing linker/C toolchain? see docs\BUILDING.md & exit /b 0 )
set "BIN=%ROOT%\target\%T%\%PROFILE%\minima%EXT%"
set "INST_ARG="
if "%INSTALLERS%"=="1" (
  set "MINIMA_PAYLOAD=%BIN%"
  %BUILD% -p minima-installer
  if errorlevel 1 ( echo !! CLI installer failed for %T% ) else ( set "INST_ARG=--installer %ROOT%\target\%T%\%PROFILE%\minima-installer%EXT%" )
  set "MINIMA_PAYLOAD="
)
"%XTASK%" dist --bin "%BIN%" --os %OS% --arch %ARCH% --version %VERSION% --out "%DIST%" %INST_ARG%
if errorlevel 1 exit /b 0
if "%INSTALLERS%"=="1" if "%OS%"=="windows" (
  call "%ROOT%\scripts\installers\windows\build-setup.bat" "%BIN%" %VERSION% %ARCH% "%DIST%"
)
set /a BUILT+=1
exit /b 0

:usage
echo Usage: scripts\build.bat [/target TRIPLE] [/all] [/noinstallers] [/debug] [/clean]
goto end

:fail
echo build failed
popd >nul
exit /b 1

:end
popd >nul
endlocal
