@echo off
setlocal EnableExtensions
chcp 936 >nul 2>&1

rem ==========================================================================
rem  media-manager 一键启动
rem  用法：
rem    双击本文件           = 构建（按需）后启动
rem    start.bat --release  = 用 release 模式构建后启动（首次较慢）
rem    start.bat --no-build = 跳过构建，直接启动已编译好的 exe
rem
rem  依赖：Node.js、Rust 工具链、MSVC 生成工具、WebView2 运行时
rem  路径可用环境变量覆盖：MM_NODE / MM_CARGO / MM_RUSTC
rem
rem  说明：脚本刻意不用 `if (...)` 代码块包裹可变命令，
rem        因为在块内 %ERRORLEVEL% 会在解析期展开，导致判错。
rem ==========================================================================

set "MM_ROOT=%~dp0"
if "%MM_ROOT:~-1%"=="\" set "MM_ROOT=%MM_ROOT:~0,-1%"

set "MM_PROFILE=release"
set "MM_DO_BUILD=1"
if /i "%~1"=="--release" set "MM_PROFILE=release"
if /i "%~1"=="--debug" set "MM_PROFILE=debug"
if /i "%~1"=="--no-build" set "MM_DO_BUILD=0"
set "MM_FORCE_FLAG="
if /i "%~1"=="--force" set "MM_FORCE_FLAG=--force"
if /i "%~2"=="--force" set "MM_FORCE_FLAG=--force"
rem 脚本会自动判断前端 / Rust 是否已最新，已最新则跳过构建（--force 强制重新构建）

rem ---- 默认路径（可用环境变量覆盖）----
if not defined MM_NODE set "MM_NODE=C:\Users\31506\.workbuddy\binaries\node\versions\22.22.2-6\node.exe"
if not exist "%MM_NODE%" if exist "C:\Users\31506\node\node-v22.23.3-win-x64\node.exe" set "MM_NODE=C:\Users\31506\node\node-v22.23.3-win-x64\node.exe"
if not defined MM_CARGO set "MM_CARGO=C:\Users\31506\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe"
if not defined MM_RUSTC set "MM_RUSTC=C:\Users\31506\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\rustc.exe"
if not defined RUSTUP_HOME set "RUSTUP_HOME=C:\Users\31506\.rustup"
if not defined CARGO_HOME set "CARGO_HOME=C:\Users\31506\.cargo"
if not defined RUSTUP_TOOLCHAIN set "RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc"

rem ---- 默认路径不存在时退回 PATH 探测 ----
if not exist "%MM_NODE%" for /f "delims=" %%i in ('where node 2^>nul') do if not defined _N set "_N=%%i"
if defined _N set "MM_NODE=%_N%"
if not exist "%MM_CARGO%" for /f "delims=" %%i in ('where cargo 2^>nul') do if not defined _C set "_C=%%i"
if defined _C set "MM_CARGO=%_C%"

rem ---- cargo 必需的环境（本机踩坑固化）----
set "CARGO_HTTP_CHECK_REVOKE=false"
set "RUSTUP_AUTO_INSTALL=0"
if exist "%MM_RUSTC%" set "RUSTC=%MM_RUSTC%"

echo.
echo ==========================================================
echo   media-manager 一键启动
echo ==========================================================
echo   项目目录 : %MM_ROOT%
echo   构建模式 : %MM_PROFILE%
echo   Node     : %MM_NODE%
echo   Cargo    : %MM_CARGO%
echo ==========================================================
echo.

if not exist "%MM_NODE%" goto :err_node
if not exist "%MM_CARGO%" goto :err_cargo

if "%MM_DO_BUILD%"=="1" call :do_build
if "%MM_DO_BUILD%"=="1" if errorlevel 1 goto :fail

set "MM_EXE=%MM_ROOT%\src-tauri\target\%MM_PROFILE%\media-manager.exe"
if not exist "%MM_EXE%" goto :err_noexe

echo.
echo [3/3] 启动应用 ...
start "" "%MM_EXE%"
echo.
echo 应用已启动。数据库位置：%%APPDATA%%\com.example.media-manager\
echo.
echo 首次使用建议顺序：
echo   1. 左侧「设置」-^> 添加媒体目录 -^> 点「保存并扫描媒体库」
echo   2. 在设置里填 TMDB API Key -^> 回列表点「批量刮削」
echo   3. 配好 PotPlayer 路径后，点海报进详情页即可播放
echo.
echo 想验证整个项目是否健康，可双击 verify.bat。
echo.
rem 成功后立即关闭控制台，只保留软件主界面（不再停留 pause 黑窗）；
rem 出错路径仍会停在控制台，便于查看错误。
exit /b 0

rem ------------------------------------------------------------------ 子过程
:do_build
pushd "%MM_ROOT%"
call :need_frontend
if errorlevel 1 goto :fe_skip
echo [1/3] 构建前端 ...
call "%MM_NODE%" node_modules\vite\bin\vite.js build
set "MM_RC=%ERRORLEVEL%"
popd
if not "%MM_RC%"=="0" goto :err_frontend
goto :rust_step

:fe_skip
echo [1/3] 构建前端 ... 已是最新，跳过
popd

:rust_step
echo.
echo [2/3] 构建 Rust（首次构建需要几分钟，请耐心等待）...
call :need_rust
if errorlevel 1 goto :rs_skip
if "%MM_PROFILE%"=="release" call "%MM_CARGO%" build --release --features custom-protocol --manifest-path "%MM_ROOT%\src-tauri\Cargo.toml"
if "%MM_PROFILE%"=="debug" call "%MM_CARGO%" build --features custom-protocol --manifest-path "%MM_ROOT%\src-tauri\Cargo.toml"
if errorlevel 1 goto :err_rust
exit /b 0

:rs_skip
echo       Rust 已是最新，跳过
exit /b 0

rem ---- 构建判定：退出码 0=需要构建，1=可以跳过 ----
:need_frontend
call "%MM_NODE%" "%MM_ROOT%\mjs\dev\need-build.mjs" --frontend %MM_FORCE_FLAG%
exit /b %ERRORLEVEL%

:need_rust
call "%MM_NODE%" "%MM_ROOT%\mjs\dev\need-build.mjs" --rust --profile %MM_PROFILE% %MM_FORCE_FLAG%
exit /b %ERRORLEVEL%

:err_node
echo [错误] 找不到 Node.js。
echo        请安装 Node.js，或设置环境变量 MM_NODE 指向 node.exe。
goto :fail

:err_cargo
echo [错误] 找不到 cargo。
echo        请安装 Rust 工具链，或设置环境变量 MM_CARGO 指向 cargo.exe。
goto :fail

:err_frontend
echo [错误] 前端构建失败，请检查上面 vite 的输出。
echo        若提示缺少依赖，先在项目目录执行：npm install
goto :fail

:err_rust
echo.
echo [错误] Rust 编译失败。常见原因：
echo        - 提示 "linker `link.exe` not found"：缺少 MSVC 生成工具。
echo          安装 Visual Studio Build Tools 并勾选「使用 C++ 的桌面开发」。
echo        - 提示网络/代理错误：可设置 CARGO_HTTP_CHECK_REVOKE=false 后重试。
echo        - 提示 "the compiler unexpectedly panicked"（ICE 编译器内部崩溃）：
echo          多半是增量编译缓存损坏。依次执行以下两条命令后重试：
echo            cd src-tauri ^&^& cargo clean
echo            cargo build --features custom-protocol
echo          （本项目已在 Cargo.toml 里关闭增量编译，原则上不会再发生）
goto :fail

:err_noexe
echo [错误] 找不到可执行文件：%MM_EXE%
echo        请先不带 --no-build 运行一次，完成构建。
goto :fail

:fail
echo.
if not defined MM_NO_PAUSE pause
exit /b 1
