@echo off
setlocal EnableExtensions
chcp 936 >nul 2>&1

rem ==========================================================================
rem  media-manager 一键盘点（验证整个项目是否健康）
rem  双击本文件即可，依次执行：
rem    1. 前端构建        vite build
rem    2. 前端类型检查    tsc --noEmit
rem    3. Rust 测试       cargo test
rem    4. Rust 编译链接   cargo build
rem  全部通过则退出码为 0。
rem
rem  本文件为 GBK 编码，请勿用 UTF-8 覆盖保存，否则 cmd 解析会出错。
rem ==========================================================================

set "MM_ROOT=%~dp0"
if "%MM_ROOT:~-1%"=="\" set "MM_ROOT=%MM_ROOT:~0,-1%"

if not defined MM_NODE set "MM_NODE=C:\Users\31506\node\node-v22.23.3-win-x64\node.exe"
if not defined MM_CARGO set "MM_CARGO=C:\Users\31506\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe"
if not defined MM_RUSTC set "MM_RUSTC=C:\Users\31506\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\rustc.exe"
if not defined RUSTUP_HOME set "RUSTUP_HOME=C:\Users\31506\.rustup"
if not defined CARGO_HOME set "CARGO_HOME=C:\Users\31506\.cargo"
if not defined RUSTUP_TOOLCHAIN set "RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc"

if not exist "%MM_NODE%" for /f "delims=" %%i in ('where node 2^>nul') do if not defined _N set "_N=%%i"
if defined _N set "MM_NODE=%_N%"
if not exist "%MM_CARGO%" for /f "delims=" %%i in ('where cargo 2^>nul') do if not defined _C set "_C=%%i"
if defined _C set "MM_CARGO=%_C%"

set "CARGO_HTTP_CHECK_REVOKE=false"
set "RUSTUP_AUTO_INSTALL=0"
if exist "%MM_RUSTC%" set "RUSTC=%MM_RUSTC%"

set "MM_FAIL=0"

echo.
echo ==========================================================
echo   media-manager 一键盘点
echo   项目目录: %MM_ROOT%
echo ==========================================================
echo.

if not exist "%MM_NODE%" goto :err_node
if not exist "%MM_CARGO%" goto :err_cargo

echo [1/4] 前端构建 (vite build)
call :fe_build
if errorlevel 1 set "MM_FAIL=1"
echo.

echo [2/4] 前端类型检查 (tsc --noEmit)
call :fe_typecheck
if errorlevel 1 set "MM_FAIL=1"
echo.

echo [3/4] Rust 单元与集成测试 (cargo test)
call :rust_test
if errorlevel 1 set "MM_FAIL=1"
echo.

echo [4/4] Rust 编译与链接 (cargo build)
call :rust_build
if errorlevel 1 set "MM_FAIL=1"
echo.

echo ==========================================================
if "%MM_FAIL%"=="0" echo   结果: 全部通过
if not "%MM_FAIL%"=="0" echo   结果: 存在失败项, 请回看上面的输出
echo ==========================================================
if exist "%MM_ROOT%\src-tauri\target\debug\media-manager.exe" echo   可执行文件: src-tauri\target\debug\media-manager.exe
echo.
echo 提示: 双击 start.bat 可直接启动应用。
echo.
if not defined MM_NO_PAUSE pause
if "%MM_FAIL%"=="0" exit /b 0
exit /b 1

rem ------------------------------------------------------------------ 子过程
:fe_build
pushd "%MM_ROOT%"
call "%MM_NODE%" node_modules\vite\bin\vite.js build
set "MM_RC=%ERRORLEVEL%"
popd
exit /b %MM_RC%

:fe_typecheck
pushd "%MM_ROOT%"
call "%MM_NODE%" node_modules\typescript\bin\tsc --noEmit
set "MM_RC=%ERRORLEVEL%"
popd
exit /b %MM_RC%

:rust_test
call "%MM_CARGO%" test --manifest-path "%MM_ROOT%\src-tauri\Cargo.toml" --quiet
exit /b %ERRORLEVEL%

:rust_build
call "%MM_CARGO%" build --manifest-path "%MM_ROOT%\src-tauri\Cargo.toml"
exit /b %ERRORLEVEL%

rem ------------------------------------------------------------------ 出错处理
:err_node
echo [错误] 找不到 Node.js。请安装 Node.js, 或设置 MM_NODE 指向 node.exe。
goto :fail

:err_cargo
echo [错误] 找不到 cargo。请安装 Rust 工具链, 或设置 MM_CARGO 指向 cargo.exe。
goto :fail

:fail
echo.
if not defined MM_NO_PAUSE pause
exit /b 1
