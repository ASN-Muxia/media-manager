@echo off
setlocal EnableExtensions
chcp 936 >nul 2>&1

rem ==========================================================================
rem  media-manager 源码打包（分发 / 传输用）
rem  用法：
rem    双击本文件        = 打成 zip（release\media-manager-src-<版本>-<短hash>.zip）
rem    package.bat gz    = 打成 tar.gz
rem    package.bat dry   = 只统计体积与条目数，不落盘
rem
rem  只打包 git 跟踪的文件，node_modules / src-tauri\target / dist / 日志
rem  一律不会进包——这是本项目压缩分发体积能下降三个数量级的关键。
rem
rem  说明：批处理必须 GBK + CRLF，否则 cmd 会把它解析成一团。
rem  默认 Node 路径可用环境变量 MM_NODE 覆盖。
rem ==========================================================================

set "MM_ROOT=%~dp0"
if "%MM_ROOT:~-1%"=="\" set "MM_ROOT=%MM_ROOT:~0,-1%"

if not defined MM_NODE set "MM_NODE=C:\Users\31506\.workbuddy\binaries\node\versions\22.22.2-3\node.exe"
if not exist "%MM_NODE%" if exist "C:\Users\31506\node\node-v22.23.3-win-x64\node.exe" set "MM_NODE=C:\Users\31506\node\node-v22.23.3-win-x64\node.exe"
if not exist "%MM_NODE%" for /f "delims=" %%i in ('where node 2^>nul') do if not defined _N set "_N=%%i"
if defined _N set "MM_NODE=%_N%"

if not exist "%MM_NODE%" goto :err_node

echo.
echo ==========================================================
echo   media-manager 源码打包
echo   项目目录 : %MM_ROOT%
echo ==========================================================
echo.

set "MM_ARGS="
if /i "%~1"=="gz" set "MM_ARGS=--format gz"
if /i "%~1"=="dry" set "MM_ARGS=--dry"
if /i "%~1"=="nogit" set "MM_ARGS=--no-git"

rem Node 输出的是 UTF-8，临时切到 65001 才能显示正常中文；
rem 跑完立刻切回 936，否则本文件剩下的 GBK 中文又会显示成一团。
chcp 65001 >nul 2>&1
call "%MM_NODE%" "%MM_ROOT%\mjs\build\package.mjs" %MM_ARGS%
set "MM_RC=%ERRORLEVEL%"
chcp 936 >nul 2>&1
if not "%MM_RC%"=="0" goto :fail

echo.
echo 打包完成。把 release 下的压缩包发给对方即可；
echo 对方解压后执行 npm install 再双击 start.bat 就能跑起来。
echo.
if not defined MM_NO_PAUSE pause
exit /b 0

:err_node
echo [错误] 找不到 Node.js。
echo        请安装 Node.js，或设置环境变量 MM_NODE 指向 node.exe。
goto :fail

:fail
echo.
if not defined MM_NO_PAUSE pause
exit /b 1
