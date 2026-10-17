# media-manager（影视库管理器）

基于 **Tauri 2 + Vue 3 + SQLite** 的本地影视库管理工具：扫描本地媒体目录、解析文件名、刮削  
TMDB 元数据、封面墙浏览、剧集分组，并可一键用 PotPlayer 播放。

---

## 一、依赖要求

| 依赖           | 版本要求                | 本机已验证版本       | 说明                                            |
| ------------ | ------------------- | ------------- | --------------------------------------------- |
| Windows      | 10 / 11             | Win11         | 仅支持 Windows（播放器桥接与打包配置都是 Windows 专用）          |
| WebView2 运行时 | 任意新版                | 152.0.4191.66 | Win11 自带；Win10 若缺失需安装 Evergreen Runtime       |
| Node.js      | ≥ 18                | v22.23.3      | 用于构建前端                                        |
| Rust 工具链     | stable（msvc）        | 1.98.1        | `rustup` 安装，须选 `x86_64-pc-windows-msvc`       |
| MSVC 生成工具    | VS 2022 Build Tools | 14.44.35207   | 必须勾选「使用 C++ 的桌面开发」，否则链接报 `link.exe not found` |
| PotPlayer    | 可选                  | —             | 未安装时会回退到系统默认播放器                               |
| TMDB API Key | 可选                  | —             | 不填也能用，只是没有海报和简介                               |

> 首次运行会自动编译 Rust 依赖（含 SQLite 与 TLS），**需要几分钟**，请耐心等待；之后是增量构建，通常几秒。

## 二、快速启动（推荐）

**三个入口指向同一个 exe**（`src-tauri\target\release\media-manager.exe`），行为一致：

| 入口                  | 怎么用           | 会不会重新构建                           |
| ------------------- | ------------- | --------------------------------- |
| `start.bat`         | 双击；看得见构建过程与报错 | ✅ 会（`vite build` + `cargo build`） |
| `scripts\启动影视库.vbs` | 双击；**全程不弹黑框** | 只在 exe 比前端旧时才静默构建                 |
| `media-manager.exe` | 直接双击二进制       | ❌ 不会（跑的是上一次构建的产物）                 |

```
start.bat                # 构建后启动（默认 release）
start.bat --debug        # 用 debug 模式构建后启动
start.bat --no-build     # 跳过构建，直接启动已有 exe
```

⚠️ **改了前端代码之后，必须让 Rust 重新编译一次**，否则双击 exe 跑的还是旧界面：  
前端产物（`dist/`）是**编译期嵌进 exe** 的。`src-tauri\build.rs` 已补了  
`cargo:rerun-if-changed=../dist`，所以只要走 `start.bat`（或 vbs 的自动构建），  
改完代码双击即可生效；**直接双击 exe 不会重新嵌入**，那一条路只适合"跑上次构建好的版本"。

### `verify.bat` —— 一键盘点

```
双击 verify.bat
```

依次执行七项检查并汇总结果，全部通过时退出码为 0：

| 步骤  | 内容                                                  |
| --- | --------------------------------------------------- |
| 1/7 | 前端构建 `vite build`                                   |
| 2/7 | 前端类型检查 `tsc --noEmit`                               |
| 3/7 | 前端单元测试 `npm test`（Vitest，429 个）                     |
| 4/7 | Rust 单元与集成测试 `cargo test`（499 个）                    |
| 5/7 | Rust 编译与链接 `cargo build --features custom-protocol` |
| 6/7 | 分层合规检查 `mjs/dev/check-layering.mjs`（禁止反向依赖）         |
| 7/7 | 打包冒烟 `mjs/build/package.mjs`                        |

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
npm test          # Vitest（jsdom），见 mjs/dev/run-tests.mjs
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
7. **手动新建剧集**：顶栏「＋ 新建剧集」，与扫描归纳出来的剧集走同一套数据；
8. **观看状态**：详情页「✓ 标记已看」/「▶ 继续播放」/「▶▶ 连播」；海报左下角有进度角标；  
   多选模式下顶栏可批量「标记已看 / 取消已看」；
9. **NFO 互通与备份**：设置页「数据互通与备份」可导出 / 导入 NFO、备份数据库、清理元数据缓存；
10. **数据源开关**：设置页「数据源开关」逐项开关 TMDb / TheTVDB / Fanart.tv / OMDb / 豆瓣；
11. **聚合分组**：顶栏「🔗 聚合」进入聚合模式 → 勾选多张后把任意一张拖到目标封面上即合并成  
    一个分组（拖到拖拽期间出现的「放回顶层」区即解除）；详情页里有「聚合成员」区，可  
    调整成员顺序、移出成员、编辑成员信息、加入新成员、撤销，以及一次性「取消聚合」。  
    **电影与剧集走完全相同的一套**（判据只看有没有子成员，不看类型）。

## 四a、观看状态的口径（必须先读这一段）

- 这是**集级**进度，**不是秒级**。
- 原因：本软件调用的是**外部播放器**（PotPlayer）。进程一旦拉起就与本应用无关，  
  播放位置**不会回传**，所以「看到第 3 分 12 秒」这种精度**本期拿不到**。
- 因此「继续播放」的语义 = **播放下一个未看集**（全部看完则从第一集重来）。
- `position_secs` 字段**预留**在库里，设置页「手动填播放位置」可用，  
  但不做任何自动采集——不做假数据。
- 想要真正的秒级续播只能内嵌播放器（自绘解码/渲染），那会改动主流程，  
  与「保持主体功能不变」的边界冲突，需单独评估后决定。

## 四c、视频文件只读红线

本项目对视频文件**只做读取**（识别格式、取路径、交给播放器），  
**任何功能都不会**转码、重编码、裁剪、覆盖写入或以任何方式改写视频文件本身。  
所有附加能力一律走旁路：

| 能力        | 落点                                        | 是否触碰视频文件 |
| --------- | ----------------------------------------- | -------- |
| 观看状态 / 续播 | `playback_state` 表                        | 否        |
| 元数据       | `media` / `episodes` 表 + 图片缓存             | 否        |
| NFO 导出    | 新建 `.nfo` 旁车文件（默认**不覆盖**已存在的）             | 否        |
| NFO 导入    | 只读 `.nfo`，只回填空字段                          | 否        |
| 技术信息探测    | `ffprobe` **只读**调用（不写 `-i` 输出）            | 否        |
| 连播        | 多个路径**直接作为参数**交给播放器，不生成临时 `.m3u8`         | 否        |
| 数据库备份     | `VACUUM INTO`，目标已存在**直接拒绝**               | 否        |
| **聚合分组**  | 只改 `media.parent_id` / `media.sort_order` | 否        |

## 四d、聚合分组（parent_id）的口径

- **只有一套层级关系**：就是 `media.parent_id` + `media.sort_order`，最多 4 层。  
  剧集层级不再单独占一个栏目或视图（`SeriesTree.vue` 已删除）——**撤掉的是入口，不是能力**。
- **入口只有两个**：① 封面墙顶栏「🔗 聚合」→ 拖封面合并；② 详情页「聚合成员」区 → 管成员与顺序。
- **电影与剧集完全一致**：后端 `load_edges` 不按类型过滤，两端同一条 `move_media_nodes`，  
  前端判据只看「有没有子成员」，没有任何 `type` 分支。
- **为什么拖动需要开「聚合」开关**：封面墙默认状态下卡片是**不可拖**的（否则按住海报划过就  
  会起拖，把「滑动框选」多选举动掐断）。开了开关才让位给原生拖拽。触摸设备起不来 HTML5 拖拽，  
  所以触摸下仍归滑动多选。
- **空聚合**（`child_count > 0` 但成员取不到）：详情页显示空态 + 「刷新」，  
  通常是成员被删了或不在当前筛选范围内，不是数据坏了。
- **取消聚合**要走确认；成员为空时**不显示**这个按钮（`ids` 为空会被后端拒绝，  
  等于给用户一条假出路），此时先「刷新」把陈旧的 `child_count` 对齐。

## 四b、元数据源与可选的大模型增强

**分工（元数据 API 是地基，AI 只是可选补漏层）：**

| 数据源       | 作用                        | 是否必需    |
| --------- | ------------------------- | ------- |
| TMDb      | 片名 / 简介 / 评分 / 海报（**权威**） | ✅ 必需    |
| TheTVDB   | 剧集的季 / 集结构（TMDb 缺集信息时兜底）  | 可选      |
| Fanart.tv | TMDb 缺图时补海报与背景            | 可选      |
| OMDb      | IMDb 评分（缺评分时才写入）          | 可选      |
| 大模型       | **只**解析脏文件名，产出「拿什么去检索」的字段 | 可选，默认关闭 |

- 可选 provider 没填 Key 就整段跳过，不影响主链路；**关掉大模型，刮削行为与没这个模块时一致**。
- 补漏而非覆盖：已有海报 / 评分一律不动。
- **数据源顺序写死**：TMDb → TheTVDB → Fanart.tv → OMDb → 豆瓣。  
  设置页「数据源开关」可以把某一源**关掉**（Key 还留着，只是不用）——  
  关掉一个源**不会**让后面的源顶上来；关掉 TMDb 时豆瓣会承担**整条刮削**，而不是直接失败。
- **元数据有本地缓存**（`provider_cache` 表，7 天 TTL）：TMDb 的检索 / 详情 / 整季单集  
  命中缓存就不再联网。设置页可「清理过期缓存」或「清空元数据缓存」。
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

| 内容         | 路径                                             |
| ---------- | ---------------------------------------------- |
| SQLite 数据库 | `%APPDATA%\com.example.media-manager\media.db` |
| 应用图标       | `src-tauri\icons\`                             |

设置页有「打开数据目录」按钮可直接定位。数据库结构版本记录在 `PRAGMA user_version`  
（当前 **v6**），升级时由 `store::schema::migrate` 自动增量补列与建表，  
**只增不删、不重建库、不丢数据**。

## 七、项目结构

```
src/                      前端（Vue 3 + Pinia，api → stores → components 三段）
  api/commands.ts         所有 Tauri 命令封装与类型
  stores/                 settingsStore / mediaStore / categoryStore / playbackStore
  components/             Sidebar / PosterGrid / PosterCard / MediaDetail /
                          SettingsPanel / MediaEditDialog / ImportFolderDialog /
                          TmdbMatchDialog / CollapsibleSection
                          （SeriesTree 已删除：层级不再单占视图，见下）
src-tauri/src/            后端（Rust，分层且禁止反向依赖）
  core/                   纯领域规则（parser / mediatype / grouping / scanner / nfo / diff）
  provider/               外部数据源（tmdb / tvdb / fanart / omdb / douban / llm /
                          probe（ffprobe 只读）/ registry（数据源注册表））
  store/                  持久化（db 连接 / schema 迁移 v6 / playback / search / cache）
  share/                  数据互通（NFO 读写）
  commands/               Tauri 命令层（唯一可同时碰上面四层）
  util/                   工具（时间 / 路径归一 / 序列化）
mjs/dev/check-layering.mjs 分层合规静态检查（core 不许依赖 store/provider/commands）
mjs/dev/dev-server.mjs    不依赖 cmd PATH 的 vite dev server 启动器
mjs/dev/run-tests.mjs     Vitest 启动器（修 watch 模式静默降级与 EPERM）
start.bat / verify.bat    快速启动与一键盘点
docs/                     调研、架构、交接与打包说明
```

**分层依赖方向（单向，不可逆）**：

```
commands ──→ core / provider / store / share / util
provider ──→ core / util
store    ──→ util
share    ──→ core / util
core     ──→ util
```

`mjs/dev/check-layering.mjs` 会在 `verify.bat` 第 6 步静态校验这一条，  
新增模块时如果写出反向依赖会直接失败。

## 八、已知限制

- 剧集分组键取「媒体根目录下的第一层目录名」，目录结构异常时退回解析标题；  
  跨媒体根目录的同一部剧不会被合并（有意为之，避免误合并）。
- 单集级刮削只覆盖库中已有季号的季；新增季需要先重新扫描。
- TheTVDB / Fanart.tv / OMDb 的调用结果**已有本地缓存**（`provider_cache` 表，7 天 TTL），  
  设置页「数据互通与备份」可清理；清理后下次刮削会重新联网取。
- 大模型增强层按文件逐个调用，脏文件名很多时扫描会明显变慢（默认关闭，可随时关掉）。
- **秒级续播做不到**（见「四a」）：外部播放器不回传进度，只能做集级续播。
- **ffprobe 是可选的**：没装 ffmpeg 时技术信息探测整段跳过，不影响其它功能；  
  设置页「维护与诊断」能看到它是否可用。
- **FTS5 可能不可用**：SQLite 是否编进 FTS5 取决于编译选项。不可用时检索自动回落到  
  `LIKE`，结果一致只是慢一点；设置页能看到当前状态。
- **增量扫描尚未启用**（`core/diff.rs` 已实现但刻意未接线）：它改的是扫描主流程，  
  且老库 `file_mtime` 恒为 NULL，「缺 mtime 一律视为变了」这条兜底需要实测验证，  
  必须单独开一个观察窗口。当前每次扫描都是全量写入（行为与升级前一致）。


- **ffprobe 探测尚未接入扫描**（`provider/probe.rs` 已实现但刻意未接线）：  
  一旦把时长喂给判定规则，R11 会第一次生效，足以把一批「待确认」翻成「电影」。  
  这是行为变更，需要单独评估。当前只在设置页的「维护与诊断」里显示 ffprobe 是否可用。
- `src-tauri/target` 约 5.9 GB（debug 全量调试信息）；`[profile.dev] incremental = false`  
  是为了修 rustc ICE，对体积的改善有限。要真正瘦身请改 `[profile.dev] debug` 或用 release 构建。
