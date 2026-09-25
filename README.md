# media-manager（影视库管理器）

基于 **Tauri 2 + Vue 3 + SQLite** 的本地影视库管理工具：扫描本地媒体目录、解析文件名、刮削
TMDB 元数据、封面墙浏览、剧集分组，并可一键用 PotPlayer 播放。

---

## 一、依赖要求

| 依赖 | 版本要求 | 本机已验证版本 | 说明 |
|---|---|---|---|
| Windows | 10 / 11 | Win11 | 仅支持 Windows（播放器桥接与打包配置都是 Windows 专用） |
| WebView2 运行时 | 任意新版 | 152.0.4191.66 | Win11 自带；Win10 若缺失需安装 Evergreen Runtime |
| Node.js | ≥ 18 | v22.23.3 | 用于构建前端 |
| Rust 工具链 | stable（msvc） | 1.98.1 | `rustup` 安装，须选 `x86_64-pc-windows-msvc` |
| MSVC 生成工具 | VS 2022 Build Tools | 14.44.35207 | 必须勾选「使用 C++ 的桌面开发」，否则链接报 `link.exe not found` |
| PotPlayer | 可选 | — | 未安装时会回退到系统默认播放器 |
| TMDB API Key | 可选 | — | 不填也能用，只是没有海报和简介 |

> 首次运行会自动编译 Rust 依赖（含 SQLite 与 TLS），**需要几分钟**，请耐心等待；之后是增量构建，通常几秒。

## 二、快速启动（推荐）

项目根目录下有两个双击即用的脚本：

### `start.bat` —— 一键启动

```
双击 start.bat              # 构建（按需）后启动
start.bat --release         # 用 release 模式构建后启动（首次较慢，产物更小更快）
start.bat --no-build        # 跳过构建，直接启动已有 exe
```

脚本会自动：
1. 按需构建前端（`vite build`，约 2 秒）；
2. 编译 Rust（增量，通常几秒）；
3. 启动 `src-tauri\target\debug\media-manager.exe`。

### `verify.bat` —— 一键盘点

```
双击 verify.bat
```

依次执行四项检查并汇总结果，全部通过时退出码为 0：

| 步骤 | 内容 |
|---|---|
| 1/4 | 前端构建 `vite build` |
| 2/4 | 前端类型检查 `tsc --noEmit` |
| 3/4 | Rust 单元与集成测试 `cargo test`（60 个） |
| 4/4 | Rust 编译与链接 `cargo build` |

### 路径覆盖（可选）

脚本内置了本机默认路径，换机器时可用环境变量覆盖，无需改脚本：

```
MM_NODE   Node.js 的 node.exe 完整路径
MM_CARGO  cargo.exe 完整路径
MM_RUSTC  rustc.exe 完整路径
```

另外两个变量：`MM_NO_PAUSE=1` 跳过结束前的「按任意键继续」（便于自动化调用）。

> ⚠️ **两个 .bat 是 GBK 编码**（中文 Windows 的 cmd 按 GBK 解析批处理文件，用 UTF-8 保存会导致
> 命令解析错乱）。若要修改，请在 GBK 编码下编辑，勿用 UTF-8 覆盖保存。

## 三、手动构建与开发

```bash
# 安装前端依赖
npm install

# 前端开发服务器（配合 src-tauri 的 devUrl: http://localhost:1420）
npm run dev

# 前端构建 / 类型检查
npm run build
npm run typecheck

# Rust 测试 / 编译
cd src-tauri
cargo test
# 生成「可直接运行」的 app：前端资源会被内嵌进二进制
cargo build --features custom-protocol
# 只做类型/借用检查（不需要内嵌资源）
cargo check

# 一体化开发（注意：beforeDevCommand 走 npm run dev，
# 若本机 cmd 的 PATH 查找失效，直接 cargo run 更稳）
cd src-tauri && cargo run
```

## 四、使用流程

1. 打开应用 → 左侧「设置」→ 点「浏览…」选择媒体目录 → 「保存并扫描媒体库」；
2. 回列表即可看到封面墙（未刮削的条目右上角有红色 `?`）；
3. 在设置里填入 TMDB API Key → 点顶部「批量刮削」，海报与简介会自动回填；
4. 点海报进详情页：可播放、重新刮削、手动匹配 TMDB、在文件夹中显示；
5. 剧集详情页按季列出单集（刮削后会显示缩略图、集名与播出日期）。

## 五、常见问题

### 窗口显示「无法访问此页面 / localhost 拒绝连接 / ERR_CONNECTION_REFUSED」

这是因为 exe 被编译成了 **dev 模式**：tauri 的 `build.rs` 里判定是
`dev = !has_feature("custom-protocol")`，没开这个 feature 时 webview 会去连
`tauri.conf.json` 里的 `devUrl`（`http://localhost:1420`），而 dev server 并没有启动。

**解决**：用带 `custom-protocol` 的方式构建 ——

```bash
cargo build --features custom-protocol
```

或者直接用 `start.bat`（它已经带上了这个参数）。
需要热重载开发时才走 `npm run tauri dev`（此时由 vite dev server 提供页面）。

### 编译报 `linker link.exe not found`

缺少 MSVC 生成工具。安装 Visual Studio Build Tools，勾选「使用 C++ 的桌面开发」。

### `cargo` / `npm` 提示「不是内部或外部命令」

本机 cmd 的 PATH 查找可能失效，请用完整路径调用，或直接用 `start.bat` / `verify.bat`（脚本内置全路径）。

## 六、数据位置

| 内容 | 路径 |
|---|---|
| SQLite 数据库 | `%APPDATA%\com.example.media-manager\media.db` |
| 应用图标 | `src-tauri\icons\` |

设置页有「打开数据目录」按钮可直接定位。数据库结构版本记录在 `PRAGMA user_version`，
升级时通过 `db::migrate` 自动增量补列，**不会丢失已有数据**。

## 七、项目结构

```
src/                      前端（Vue 3 + Pinia）
  api/commands.ts         所有 Tauri 命令封装与类型
  stores/                 settingsStore / mediaStore
  components/             Sidebar / PosterGrid / PosterCard / MediaDetail / SettingsPanel
src-tauri/src/            后端（Rust）
  db.rs                   连接、建表、增量迁移
  parser.rs               文件名 -> 标题/年份/季/集/TMDB id
  scanner.rs              目录扫描与入库（剧集按目录分组）
  tmdb.rs                 TMDB v3 客户端
  commands/               settings / media / scrape / play 四组命令
start.bat / verify.bat    快速启动与一键盘点
交接状态-2026-09-25.md     详细的开发与验证记录
```

## 八、已知限制

- 前端只有构建 + 类型检查两道验证，没有组件级单元测试；
- 剧集分组键取「媒体根目录下的第一层目录名」，若目录结构异常会退回解析标题；
- 目前只支持 TMDB 一个元数据源，且仅中文（`zh-CN`）；
- 扫描是同步执行，媒体库很大时界面会等待且没有进度条。
