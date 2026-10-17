@echo off
setlocal EnableExtensions
chcp 936 >nul 2>&1

rem ==========================================================================
rem  media-manager 一键启动（唯一入口 · 2026-10-17 合并版）
rem
rem  由原 start.bat 与 scripts\启动影视库.vbs 合并而来，行为：
rem    * 按需构建：判据是「内容指纹」（mjs\dev\need-build.mjs），改测试文件 /
rem      编辑器 touch / 云盘或杀软刷时间戳都不再触发无谓构建；构建成功后
rem      自动写指纹（--commit），下次双击直接秒开。
rem    * 构建前自动清掉残留实例（原 vbs 的 KillStale），避免链接「拒绝访问」
rem      与多实例抢 WebView2 数据目录。
rem    * 成功启动后控制台立即关闭，只留主界面；出错才停留显示原因。
rem
rem  用法：
rem    双击本文件            = 按需构建后启动（默认 release，无改动则秒开）
rem    start.bat --no-build  = 跳过构建判定，直接启动已有 exe
rem    start.bat --debug     = 用 debug 模式构建后启动
rem    start.bat --force     = 忽略指纹，强制完整重建
rem
rem  依赖：Node.js、Rust 工具链、MSVC 生成工具、WebView2 运行时
rem  路径可用环境变量覆盖：MM_NODE / MM_CARGO / MM_RUSTC
rem
rem  ? 本文件必须保持 GBK + CRLF，否则 cmd 会把它解析成一团。
rem ==========================================================================

set "MM_ROOT=%~dp0"
if "%MM_ROOT:~-1%"=="\" set "MM_ROOT=%MM_ROOT:~0,-1%"

set "MM_PROFILE=release"
set "MM_DO_BUILD=1"
set "MM_FORCE_FLAG="
if /i "%~1"=="--debug" set "MM_PROFILE=debug"
if /i "%~1"=="--no-build" set "MM_DO_BUILD=0"
if /i "%~1"=="--force" set "MM_FORCE_FLAG=--force"
if /i "%~2"=="--force" set "MM_FORCE_FLAG=--force"

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

set "MM_EXE=%MM_ROOT%\src-tauri\target\%MM_PROFILE%\media-manager.exe"

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

if "%MM_DO_BUILD%"=="0" goto :launch
if not exist "%MM_NODE%" goto :err_node
if not exist "%MM_CARGO%" goto :err_cargo

rem ---------------------------------------------------------------- 按需构建
rem need-build 退出码 0 = 需要构建（同时打印原因）；1 = 已是最新。
set "MM_NEED_FE=0"
set "MM_NEED_RS=0"
call "%MM_NODE%" "%MM_ROOT%\mjs\dev\need-build.mjs" --frontend %MM_FORCE_FLAG%
if not errorlevel 1 set "MM_NEED_FE=1"
call "%MM_NODE%" "%MM_ROOT%\mjs\dev\need-build.mjs" --rust --profile %MM_PROFILE% %MM_FORCE_FLAG%
if not errorlevel 1 set "MM_NEED_RS=1"

if "%MM_NEED_FE%"=="0" if "%MM_NEED_RS%"=="0" goto :launch

rem 要构建就先清残留实例：占住 exe 会让链接失败或变慢（实测报「拒绝访问」），
rem 多实例抢同一个 WebView2 数据目录还会误触发「启动异常」弹窗。
taskkill /F /IM media-manager.exe >nul 2>nul

if "%MM_NEED_FE%"=="0" goto :rust_step
echo [1/2] 构建前端 (vite build) ...
pushd "%MM_ROOT%"
call "%MM_NODE%" node_modules\vite\bin\vite.js build
set "MM_RC=%ERRORLEVEL%"
popd
if not "%MM_RC%"=="0" goto :err_frontend
call "%MM_NODE%" "%MM_ROOT%\mjs\dev\need-build.mjs" --frontend --commit

:rust_step
if "%MM_NEED_RS%"=="0" goto :launch
echo [2/2] 构建 Rust (cargo build --%MM_PROFILE%，首次需要几分钟，请耐心等待) ...
if "%MM_PROFILE%"=="release" call "%MM_CARGO%" build --release --features custom-protocol --manifest-path "%MM_ROOT%\src-tauri\Cargo.toml"
if "%MM_PROFILE%"=="debug" call "%MM_CARGO%" build --features custom-protocol --manifest-path "%MM_ROOT%\src-tauri\Cargo.toml"
if errorlevel 1 goto :err_rust
call "%MM_NODE%" "%MM_ROOT%\mjs\dev\need-build.mjs" --rust --profile %MM_PROFILE% --commit

:launch
if not exist "%MM_EXE%" goto :err_noexe
echo.
echo 启动应用 ...
start "" "%MM_EXE%"
echo 完成。数据库位置：%%APPDATA%%\com.example.media-manager\
echo.
echo 首次使用建议顺序：
echo   1. 左侧「设置」-^> 添加媒体目录 -^> 点「保存并扫描媒体库」
echo   2. 在设置里填 TMDB API Key -^> 回列表点「批量刮削」
echo   3. 配好 PotPlayer 路径后，点海报进详情页即可播放
echo.
echo 想验证整个项目是否健康，可双击 verify.bat；要打源码分发包，双击 package.bat。
echo.
rem 成功后立即关闭控制台，只保留软件主界面（不再停留 pause 黑窗）；
rem 出错路径仍会停在控制台，便于查看错误。
exit /b 0

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
