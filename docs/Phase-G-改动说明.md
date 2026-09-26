# Phase G 改动说明

日期：2026-09-26
范围：`G:\AI agent\media-manager`（Tauri 2 + Vue 3 + rusqlite）

三项改造全部完成并通过全量验证（`verify.bat` 5/5 全绿，`cargo test` **235 passed / 0 failed**，0 warning）。

---

## 一、影视作品文件夹优先识别

### 目标

分类与聚集流程改为**先认出代表一部作品的目录**，再以该目录为聚合单位收纳其中的季、集、特辑，
避免文件被逐个散落归类。

### 改动位置

**`src-tauri/src/scanner.rs`**

| 位置 | 说明 |
|---|---|
| `work_dir_of()`（新增，L219） | 取代原 `top_folder_of()`。挑选顺序：① 相对扫描根的**第一层目录**，若它像个作品名就用它；② 第一层是纯结构名（`Season 1` / `S01`）→ 说明用户把作品目录本身当扫描根，改用扫描根的目录名；③ 两者都不像 → 退回第一层，保持原行为。 |
| `collect_files_reported()`（L301） | 调用点改为 `let folder = work_dir_of(root_path, entry.path());` |
| `classify_files()`（L106） | 调用 `fill_episode_numbers(&mut f.parsed, &f.name, parent_name.as_deref())`（新签名，见 §二） |
| `apply_group_plan()`（L176） | `GroupInput` 新增 `parent` 字段，把文件**所在目录名**送进分组决策 |

**`src-tauri/src/grouping.rs`**

| 位置 | 说明 |
|---|---|
| `NOT_A_WORK`（新增，L243） | 「显然是容器而非作品」的目录名清单：分类层（`电视剧` / `电影` / `动漫` / `TV` / `Movies`）、光盘分片（`CD` / `Disc` / `Part` / `Vol`）、占位名（`新建文件夹` / `未命名`） |
| `looks_like_work_name()`（新增，L254） | 归一化后为空（`Season 1` / `S01` 这类结构名）→ 不是作品；去尾数字后命中 `NOT_A_WORK` → 不是作品 |
| `belongs_to_parent()`（新增，L469） | 簇所在的**目录名**与主组归一标题相似（≥ `MERGE_THRESHOLD`）→ 认它属于主组 |
| `strip_release_tag()`（新增，L487） | 只在比对目录名时剥掉 `[...]` 后缀与 `-GROUP` 尾巴。不影响标题归一化，`X-Men` / `Spider-Man` 该怎么比还怎么比 |
| `has_title_conflict()`（新增，L511） | **替换**旧的 `b.numbered > b.episodes.len()` 判定。冲突定义为「同一个 (季,集) 上站着两个**互不相似**的归一标题」——同一季的重复压制件也会撞号，但它们明明是同一部剧，旧判定会误拆 |
| `split_clusters()`（新增，L537） | 按相似度分簇，人多的簇当主组；其余簇只有在「≥ `MIN_SPLIT_MEMBERS`(2) 个带真实编号的成员」**且** `!belongs_to_parent(...)` 时才保留，否则并回主组 |

**为什么不会因为改名 / 发行组前缀而被拆开**

文件名可能被发行组弄得面目全非（`trips-doctor.who.s07.chrimbo.mkv`），但它住的目录通常是干净的
（`Doctor.Who.2005.2012.Christmas.Special...\`）。`strip_release_tag` + `belongs_to_parent`
把「住在同名的发行目录里」变成了最强的一条归并证据。

### 改动前后对比

真实扫描根 `D:\电视剧`：

| 指标 | 改动前 | 改动后 |
|---|---|---|
| 聚合单位 | 逐个文件各自归类 | 先认作品目录，目录内季/集/特辑统一归入该作品 |
| 神秘博士 | 拆成约 10 条独立条目（含 4 条 `movie`） | **1 个剧集组** |

新增回归测试：

- `scanner.rs::work_folder_is_the_aggregation_unit_when_root_is_the_show_dir`（扫描根即作品目录，`Season 1/2` + `Specials` → 1 组 4 集，组键 `Legal High`）
- `grouping.rs::polluted_filenames_are_pulled_back_by_their_directory`（文件名被污染但目录干净）
- `grouping.rs::mixed_folder_still_splits_when_directory_says_nothing`（目录什么都说明不了时，真混了两部剧仍然要拆）
- `grouping.rs::work_name_detection_skips_containers_and_categories`

---

## 二、Doctor Who 圣诞特辑归属问题

### 现象

圣诞特辑没有并入《神秘博士》正剧，而是各自成为独立条目，其中 4 条还被判成 `movie`
（旧库 id 10972–10975，路径均在 `…\S07…\Specials\` 下），组键长得像
「神秘博士 · doctor who christmas special」「…the next doctor」「…a christmas carol special」
等互不相关的一串。

### 根因定位（四个具体缺陷）

| # | 位置（改动前） | 问题 |
|---|---|---|
| 1 | `mediatype.rs::re_special` | 正则只有中文与 `ova/oad/sp`，**没有英文 `special` / `christmas`**。特辑文件名 `Doctor.Who.2005.Christmas.Special.2015...` 命不中 R3，拿不到「这是剧集体系里的特别篇」这一票 |
| 2 | `grouping.rs::NOISE_TOKENS` | 只有中文单集类型词，**没有 `special` / `specials` / `christmas`**。于是 `doctor who christmas special` 归一后仍是 `doctor who christmas special`，与正剧 `doctor who` 的相似度 ≈ 0.5，**远低于**合并阈值 `MERGE_THRESHOLD = 0.9`，永远合不进去 |
| 3 | `mediatype.rs::classify` R9 | R9 只认目录名里的**季标记**（`Season 7` / `S07`）。`Specials\` 目录里一个季标记都没有，这些文件在打分时**拿不到任何剧集票**，默认判定把它们推给了 `movie` |
| 4 | `mediatype.rs::fill_episode_numbers` | 函数签名只有 `(parsed, file_name)`，**不知道文件住在哪个目录**。特辑文件自己没有 S00/E00 标记（例如 `The.Time.of.the.Doctor.mkv`），只能被补默认季号 1，于是和正剧第 1 季撞号——即使合进组了也是错位的 |

补充：所谓「S00/E00 分支条件」本身没错，`season = Some(0)` 的逻辑一直是对的；
**真正的问题是它从未被触发**——触发条件依赖第 1 条缺失的英文正则。

### 修复点

**`src-tauri/src/mediatype.rs`**

| 位置 | 改动 |
|---|---|
| `re_special`（L353） | 改为 `(?i)\b(ova\|oad\|sp\|specials?\|christmas)\b\|特别篇\|番外篇\|番外\|特别版\|特辑` |
| `re_dir_special` + `dir_has_special_marker()`（新增，L370 / L401） | 认「特辑目录」：`Specials?` / `Extras?` / `Featurettes?` / `Bonus` / `OVA` / `OAD` / `特辑` / `特别篇` / `番外` |
| `classify` R9（L538） | 增加第二分支：父目录命中特辑目录 → `tv += w.season_folder`（理由文案「位于特辑 / Extras 目录」） |
| `fill_episode_numbers`（L655） | 签名改为 `(parsed, file_name, parent: Option<&str>)`；`special = is_special(file_name) \|\| parent.map(dir_has_special_marker)...`，特辑一律归入**第 0 季**（文件名里写明的季号仍然尊重，靠 `season_marker` 判定） |

**`src-tauri/src/grouping.rs`**

| 位置 | 改动 |
|---|---|
| `NOISE_TOKENS`（L65） | 补 `"special", "specials", "christmas", "extras", "sample", "proof"` |

**`src-tauri/src/commands/media.rs`**

| 位置 | 改动 |
|---|---|
| `convert_to_episode()` | 适配新签名：`mediatype::fill_episode_numbers(&mut parsed, &name, Some(&dir_name_of(&row.file_path)))`，保证入库路径与扫描路径行为一致 |

### 改动前后对比

| 用例 | 改动前 | 改动后 |
|---|---|---|
| 神秘博士 + 圣诞特辑（真实 18 个文件） | 约 10 条独立条目，其中 4 条 `movie` | **1 个剧集组 / 18 集**：`(0,1)…(0,11)` 为圣诞特辑，`(1,1)(1,2)(7,1)(7,2)(8,1)(8,2)(8,3)` 为正剧 |
| 全盘真实扫描 `D:\电视剧` | — | 564 个视频 → Tv 451 / Movie 2 / Unknown 111；**神秘博士收敛为 1 组 173 集**，含 2012 圣诞特辑 `TRiPS` 发行目录（靠 `strip_release_tag` + `belongs_to_parent` 收回） |

新增回归测试：

- `scanner.rs::real_doctor_who_christmas_specials_join_the_main_series`（18 个真实文件名端到端）
- `mediatype.rs::english_christmas_special_is_tv`
- `mediatype.rs::specials_folder_makes_unmarked_files_tv`
- `mediatype.rs::specials_are_numbered_into_season_zero`
- `grouping.rs::normalize_strips_english_episode_type_words`

---

## 三、视频格式识别补全

### 3.1 D 盘视频扩展名统计结果

全盘递归扫描 `D:\`（跳过 `$RECYCLE.BIN` / `System Volume Information` / `RECYCLER` / `lost+found`），
耗时 0.1 秒。原始数据落盘在 **`docs/scan_exts_d.json`**，采集脚本 `scripts/scan_video_exts.py`。

- 文件总数 **2453**，其中**无扩展名 32 个**
- 按文件头确认为真视频的：**1566 个**

出现过的全部扩展名（Top 30）：

| 扩展名 | 数量 | 扩展名 | 数量 | 扩展名 | 数量 |
|---|---|---|---|---|---|
| `.mp4` | 1145 | `.rmvb` | 19 | `.db` | 9 |
| `.mkv` | 370 | `.flac` | 19 | `.db-journal` | 6 |
| `.torrent` | 284 | `.png` | 13 | `.exe` | 6 |
| `.dll` | 90 | `.zip` | 13 | `.wav` | 5 |
| `.srt` | 77 | `.txt` | 10 | `.bin` | 4 |
| `.ass` | 68 | `.flv` | 10 | `.pfl` | 4 |
| `.jp2` | 68 | `.ape` | 10 | `.rar` | 4 |
| `.pak` | 58 | `.log` | 3 | `.docx` | 2 |
| `.mp3` | 42 | `.idx` | 22 | `.dat` | 2 |
| `.sub` | 22 | `.jpg` | 19 | `.avi` | 1 |

其余各 1 个：`.skin` / `.ntpk` / `.ini` / `.ncae` / `.ico` / `.vdf` / `.doc` / `.mka` / `.cue` /
`.nfo` / `.srr` / `.tmp` / `.msdkvideoindex` / `.pek`

**真实视频容器只有 5 种**：`.mp4` 1145、`.mkv` 370、`.rmvb` 19、`.flv` 10、`.avi` 1

**大小写情况**（只看已知视频扩展名的变体）：`.mp4` 1107、`.mkv` 370、`.MP4` **38**、`.rmvb` 19、
`.flv` 10、`.dat` 2、`.avi` 1 —— **38 个大写 `.MP4` 证明了忽略大小写是必需的**。

**刻意排除的两类**：

- `.dat`（2 个）是网易云音乐的程序数据（`icudtl.dat` / `isc_chat.dat`），文件头不满足任何视频容器签名。VCD 时代的视频 `.dat` 与它同后缀，收进来就是实打实的误判。
- `.sub`（22 个）是 VobSub 字幕，其 pack 头 `00 00 01 BA` 与 MPEG-PS 视频共用——这个事实直接决定了下面文件头识别里**不做 MPEG-PS**。

### 3.2 扩展清单修正

**文件：`src-tauri/src/parser.rs`**

| 位置 | 改动 |
|---|---|
| `VIDEO_EXTS`（L28） | 17 → **23** 条。新增 `.mk3d`（Matroska 3D）、`.f4v`（Flash MP4）、`.divx`（DivX AVI）、`.mts`（AVCHD 流）、`.m2v`（纯视频 ES）、`.asf`（ASF 家族）。这 6 条与实测容器**同族**，且都在 `video_signature()` 的覆盖范围内，用来兜住用户的其他盘 |
| `is_video_file`（L40） | 保留原实现（已 `to_ascii_lowercase()` 后比对），补注释说明 D 盘实测有 38 个 `.MP4` |

**不改变**：原有 17 条一个没删，`.dat` 明确不收。

### 3.3 文件头（magic number）兜底

**文件：`src-tauri/src/parser.rs`（新增）**

| 符号 | 说明 |
|---|---|
| `enum VideoContainer`（L47） | `Matroska` / `IsoBmff` / `Avi` / `FlashVideo` / `RealMedia` / `Asf` / `MpegTs` |
| `MIN_PROBE_BYTES`（L61） | 512 KB。低于此体积的「无名文件」不读盘——一轮全盘扫描里绝大多数异常文件是几十 KB 的程序数据 |
| `MKV_VIDEO_CODECS`（L65） | 13 个视频 codec ID。`.mka`（纯音频）与视频共用 EBML 头，只能靠这个区分 |
| `AUDIO_ONLY_BRANDS`（L71） | `M4A ` / `M4B ` / `mp42audio`，ISOBMFF 里只装音频的品牌要排除 |
| `video_signature(head)`（L81） | 认 ISOBMFF `ftyp` 盒、Matroska EBML + 视频 codec、RIFF/AVI、FLV、`.RMF` RealMedia、ASF GUID、MPEG-TS（要求第 0/188/376 字节连续三个同步字节 `0x47` 才认）。**刻意不做 MPEG-PS**——见 §3.1 的 `.sub` 说明 |
| `looks_like_video_file(path, size)`（L117） | 只读前 64 KiB；打开失败 / 读失败一律当「不是视频」，不重试不报错 |

**文件：`src-tauri/src/scanner.rs`**

| 位置 | 改动 |
|---|---|
| `collect_files_reported()`（L296） | 兜底接线：`if !is_video_file(&name) && !parser::looks_like_video_file(entry.path(), size) { continue; }`。先用 `entry.metadata()` 拿体积（几乎零成本），扩展名认得出来就直接过，只有「认不出来」的才读盘 |

### 3.4 不误伤的验证

新增单测锁定边界：

- `video_ext_whitelist`：大写合法（`c.MP4` / `d.RmVB` / `e.Mp4`），负例 `icudtl.dat`、`Movie.mkv.part` 不认
- `video_ext_covers_same_container_families`：同族扩展名覆盖
- `header_signature_recognizes_video_containers`：7 种容器签名正例
- `header_signature_rejects_sub_and_audio_only`：`.sub` 与纯音频品牌必须被拒

真实数据交叉校验：对全盘 2453 个文件跑纯文件头识别，得到 **1566 个真视频**，
扩展名分布 `.mp4` 1144 / `.mkv` 369 / `.rmvb` 19 / `.flv` 10 / `.avi` 1 / `.sub` 22 / `.mka` 1 ——
后两项正是被 `video_signature()` 设计排除的假阳性点。

---

## 四、验证

```
[1/5] vite build                    ✅ built in 1.66s（61 modules）
[2/5] tsc --noEmit                  ✅ 0 错误
[3/5] vitest run                    ✅ 8 files / 79 passed
[4/5] cargo test                    ✅ 235 passed / 0 failed / 0 ignored
[5/5] cargo build                   ✅ Finished dev profile in 3.07s
结果: 全部通过
```

Rust 测试数 223 → **235**（新增 12 项）。编译 0 warning。

## 五、改动文件清单

| 文件 | 性质 |
|---|---|
| `src-tauri/src/parser.rs` | 改（`VIDEO_EXTS` 扩容 + 文件头识别） |
| `src-tauri/src/scanner.rs` | 改（作品目录识别、文件头兜底接线、`GroupInput.parent`） |
| `src-tauri/src/grouping.rs` | 改（作品名判定、目录归属证据、冲突判定重写、噪声词） |
| `src-tauri/src/mediatype.rs` | 改（英文特辑正则、特辑目录、R9 新分支、`fill_episode_numbers` 签名） |
| `src-tauri/src/commands/media.rs` | 改（适配新签名） |
| `scripts/scan_video_exts.py` | 新增（D 盘扫描脚本） |
| `docs/scan_exts_d.json` | 新增（统计结果原始数据） |
| `docs/Phase-G-改动说明.md` | 新增（本文档） |

临时探针模块 `mod tmp_real_probe` 及备份目录 `.tmp-phaseg/` 已在取证结束后**全部移除**，
源码树不含任何调试残留。

## 六、已知限制

- 极少数真正无扩展名的视频若小于 512 KB，会被 `MIN_PROBE_BYTES` 挡掉（刻意换取扫描性能）。
- `.dat` 未收：若其他盘存在 VCD 镜像，需按实际情况再评估。
- 依赖目录名正确：若用户把整季文件直接摊在扫描根目录下（`D:\电视剧\01.mkv`），
  `work_dir_of` 拿不到作品信息，行为与改动前一致。
