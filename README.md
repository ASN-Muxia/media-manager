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

依次执行五项检查并汇总结果，全部通过时退出码为 0：

| 步骤 | 内容 |
|---|---|
| 1/5 | 前端构建 `vite build` |
| 2/5 | 前端类型检查 `tsc --noEmit` |
| 3/5 | 前端单元测试 `npm test`（Vitest） |
| 4/5 | Rust 单元与集成测试 `cargo test`（150 个） |
| 5/5 | Rust 编译与链接 `cargo build --features custom-protocol` |

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

# 前端构建 / 类型检查 / 单元测试
npm run build
npm run typecheck
npm test          # Vitest（jsdom），见 scripts/run-tests.mjs
npm run test:watch

# Rust 测试 / 编译
cd src-tauri
cargo test
# 生成「可直接运行」的 app：前端资源会被内嵌进二进制
cargo build --features custom-protocol
# 只做类型/借用检查（不需要内嵌资源）
cargo check

# 一体化开发（beforeDevCommand 已置空，dev server 用下面的脚本起，
# 它直接调 vite 的 Node API，不依赖 cmd 的 PATH 查找）
npm run dev:server        # 起 http://localhost:1420
cd src-tauri && cargo run
```

## 四、使用流程

1. 打开应用 → 左侧「设置」→ 点「浏览…」选择媒体目录 → 「保存并扫描媒体库」；
2. 回列表即可看到封面墙（未刮削的条目右上角有红色 `?`）；
3. 在设置里填入 TMDB API Key → 点顶部「批量刮削」，海报与简介会自动回填；
4. 点海报进详情页：可播放、重新刮削、手动匹配 TMDB、在文件夹中显示；
5. 剧集详情页按季列出单集（刮削后会显示缩略图、集名与播出日期）；
6. **分类与标签**：左侧「分类」区的 ＋ 可新建栏目，hover 栏目可改名 / 删除；
   详情页里勾选分类、添加标签；顶栏有标签下拉筛选；
7. **手动新建剧集**：顶栏「＋ 新建剧集」，与扫描归纳出来的剧集走同一套数据。

## 四b、元数据源与可选的大模型增强

**分工（元数据 API 是地基，AI 只是可选补漏层）：**

| 数据源 | 作用 | 是否必需 |
|---|---|---|
| TMDb | 片名 / 简介 / 评分 / 海报（**权威**） | ✅ 必需 |
| TheTVDB | 剧集的季 / 集结构（TMDb 缺集信息时兜底） | 可选 |
| Fanart.tv | TMDb 缺图时补海报与背景 | 可选 |
| OMDb | IMDb 评分（缺评分时才写入） | 可选 |
| 大模型 | **只**解析脏文件名，产出「拿什么去检索」的字段 | 可选，默认关闭 |

- 可选 provider 没填 Key 就整段跳过，不影响主链路；**关掉大模型，刮削行为与没这个模块时一致**。
- 补漏而非覆盖：已有海报 / 评分一律不动。
- 推荐模型：文本 `deepseek-chat`（`https://api.deepseek.com/v1`），视觉 `qwen-vl-max`；
  都走 OpenAI 兼容协议（备选：OpenAI / Moonshot / 智谱 / 本地 Ollama），设置页可一键填入。
- 调用失败**自动回退**到正则解析器，不会让识别结果变糟。
- 视觉模型只在文件名**完全不可推断**（纯数字 / 纯符号）时才会被调用，后端有闸门。
- **不得以纯 AI 刮削替代元数据 API**：标题、简介、评分、海报一律以元数据 API 为准。

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

### 编译报 `the compiler unexpectedly panicked`（ICE 编译器内部崩溃）

**这不是代码 bug**，是增量编译缓存损坏（本机 exe 运行中被锁/杀软干扰导致增量文件半写入，
叠加反复切换 feature 所致）。恢复方法：

```bash
cd src-tauri
cargo clean
cargo build --features custom-protocol
```

本项目已在 `Cargo.toml` 里用 `[profile.dev] incremental = false` 永久关闭增量编译，
原则上不会再发生；完整编译仍是秒级。

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
  stores/                 settingsStore / mediaStore / categoryStore
  components/             Sidebar / PosterGrid / PosterCard / MediaDetail /
                          SettingsPanel / CreateSeriesDialog
src-tauri/src/            后端（Rust）
  db.rs                   连接、建表、增量迁移（当前 v4，8 张表）
  parser.rs               文件名 -> 标题/年份/季/集/TMDB id
  grouping.rs             剧集识别与二次校正（防误判规则都在这里）
  scanner.rs              目录扫描与入库（L0 按目录归集 + 二次校正 + 可选 LLM 增强）
  tmdb.rs / tvdb.rs / fanart.rs / omdb.rs   四个元数据客户端
  http.rs                 共享 HTTP 助手（hosts 自定义解析、错误标签）
  llm.rs                  大模型脏文件名解析（可选增强层，失败即回退）
  commands/               settings / media / catalog / scrape / play / diag
scripts/dev-server.mjs    不依赖 cmd PATH 的 vite dev server 启动器
scripts/run-tests.mjs     Vitest 启动器（修 watch 模式静默降级与 EPERM）
start.bat / verify.bat    快速启动与一键盘点
交接状态-2026-09-25.md     详细的开发与验证记录
```

## 八、已知限制

- 剧集分组键取「媒体根目录下的第一层目录名」，目录结构异常时退回解析标题；
  跨媒体根目录的同一部剧不会被合并（有意为之，避免误合并）。
- 单集级刮削只覆盖库中已有季号的季；新增季需要先重新扫描。
- TheTVDB / Fanart.tv / OMDb 与大模型的调用结果**没有本地缓存**，重复刮削会重复请求。
- 大模型增强层按文件逐个调用，脏文件名很多时扫描会明显变慢（默认关闭，可随时关掉）。
- `src-tauri/target` 约 5.9 GB（debug 全量调试信息）；`[profile.dev] incremental = false`
  是为了修 rustc ICE，对体积的改善有限。要真正瘦身请改 `[profile.dev] debug` 或用 release 构建。
