//! 文件名解析：从视频文件名推断 标题 / 年份 / 季 / 集 / TMDB id。
//!
//! 覆盖常见命名：
//! - `The.Matrix.1999.1080p.BluRay.x264.mkv`
//! - `Inception (2010) [1080p].mkv`
//! - `Show.Name.S01E02.1080p.WEB-DL.mkv`
//! - `Show Name - S02E10 - Episode Title.mkv` / `Show Name - 01.mkv`
//! - `Doctor Who Season 4 Episode 3.mkv`
//! - `剧名.第一季.E01.2021.1080p.mkv`
//! - `[字幕组] 剧名 - 01 [1080P][简体].mp4`
//! - `动画名 第01话.mp4`
//! - `Movie.2021.2160p.WEB-DL.DDP5.1.HDR.x265-GROUP.mkv`

use crate::util::lazy_re;
use regex::Regex;
use std::sync::OnceLock;

/// 视频扩展名白名单（**小写**，含点；匹配时统一转小写后再比对）。
///
/// 清单依据：本机 D 盘全盘实测（见 `docs/scan_exts_d.json`）。盘上真实存在的
/// 视频只有 `.mp4` / `.mkv` / `.rmvb` / `.flv` / `.avi` 五种，它们在这份清单里；
/// 其余条目是**与实测容器同族**的常见写法，用来兜住用户的其他盘。
///
/// 刻意**没有**收录的：
/// - `.dat`：VCD 容器的扩展名，但 D 盘上仅有的 2 个 `.dat` 是网易云音乐的程序
///   数据（`icudtl.dat` / `isc_chat.dat`，文件头不满足任何视频容器签名）。
///   收进来就是实打实的误判，所以不收。
/// - `.mkv.x` 这类进度的临时后缀、以及 `.srt` / `.ass` 等字幕：不是视频。
pub const VIDEO_EXTS: &[&str] = &[
    ".mkv", ".mp4", ".avi", ".rmvb", ".rm", ".mov", ".wmv", ".flv", ".ts", ".m2ts",
    ".mpg", ".mpeg", ".m4v", ".webm", ".vob", ".iso", ".3gp",
    // 与实测容器同族的写法：Matroska 3D / Flash MP4 / DivX AVI / AVCHD 流 /
    // 纯视频 ES / ASF 家族。文件头识别（`video_signature`）覆盖的也是这一批容器。
    ".mk3d", ".f4v", ".divx", ".mts", ".m2v", ".asf",
];

/// 判断是否为受支持的视频文件（**忽略大小写**）。
///
/// 只看扩展名，不读文件内容——盘上确实存在 `.MP4` 这种大写写法（D 盘上有 38 个），
/// 所以必须先把整个名字转小写再比对。
pub fn is_video_file(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    VIDEO_EXTS.iter().any(|e| lower.ends_with(e))
}

/// 文件容器签名。**扩展名缺失或异常**时的兜底依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoContainer {
    Matroska,
    IsoBmff,
    Avi,
    FlashVideo,
    RealMedia,
    Asf,
    MpegTs,
}

/// 兜底判定的最小体积：小于这个大小的「无名文件」不读盘。
///
/// 一轮全盘扫描里绝大多数异常文件是程序数据（几十 KB 的配置、邮件分片），
/// 而一个视频再小也得以百 KB 计；设个门槛可以省掉大量无效 IO。
pub const MIN_PROBE_BYTES: u64 = 512 * 1024;

/// 文件里的视频轨（`Matroska` 容器用）：`.mka`（纯音频）与视频共用 EBML 头，
/// 只能靠 codec ID 区分。
const MKV_VIDEO_CODECS: &[&[u8]] = &[
    b"V_MPEG", b"V_VP8", b"V_VP9", b"V_AV1", b"V_THEORA", b"V_MS/VFW", b"V_PRORES",
    b"V_REAL", b"V_QUICKTIME", b"V_UNCOMPRESSED", b"V_FFV1", b"V_DIRAC", b"V_SNOW",
];

/// ISOM 品牌里只装音频的几种（`.m4a` 之类），视频不该认它们。
const AUDIO_ONLY_BRANDS: &[&[u8]] = &[b"M4A ", b"M4B ", b"mp42audio"];

/// 从文件开头的内容认容器。
///
/// 只认**确定能自证**的签名，宁可漏也不误判：
/// - ISOBMFF（`mp4` / `m4v` / `mov`）看 `ftyp` 盒子；
/// - Matroska 头必须能在开头这段里找到视频 codec ID，否则它是 `.mka` / `.mks`；
/// - **刻意不做 MPEG-PS（`00 00 01 BA`）**：VobSub 字幕（`.sub`）用的就是这套
///   pack 头，D 盘上 22 个 `.sub` 会被误判成视频。这类容器本来就有明确的
///   扩展名，没必要在无扩展名的场景里冒险。
pub fn video_signature(head: &[u8]) -> Option<VideoContainer> {
    if head.len() >= 12 && head[4..8] == *b"ftyp" {
        let brand = &head[8..12];
        if AUDIO_ONLY_BRANDS.iter().any(|b| brand == *b) {
            return None;
        }
        return Some(VideoContainer::IsoBmff);
    }
    if head.starts_with(b"\x1a\x45\xdf\xa3") {
        let has_video = MKV_VIDEO_CODECS
            .iter()
            .any(|c| head.windows(c.len()).any(|w| w == *c));
        return has_video.then_some(VideoContainer::Matroska);
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"AVI " {
        return Some(VideoContainer::Avi);
    }
    if head.starts_with(b"FLV\x01") || head.starts_with(b"FLV\x00") {
        return Some(VideoContainer::FlashVideo);
    }
    if head.starts_with(b".RMF") {
        return Some(VideoContainer::RealMedia);
    }
    if head.len() >= 8 && head[0..8] == [0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11] {
        return Some(VideoContainer::Asf);
    }
    // MPEG-TS：188 字节一个包，同步字节要不连两个都对齐才算
    if head.len() >= 376 && head[0] == 0x47 && head[188] == 0x47 && head[376] == 0x47 {
        return Some(VideoContainer::MpegTs);
    }
    None
}

/// 读一段文件头判断是不是视频（**只给「没有扩展名 / 扩展名异常」的文件用**）。
///
/// 只读前 64 KiB；读不出来（权限、被占用）一律当不是视频，不重试、不报错。
pub fn looks_like_video_file(path: &std::path::Path, size: u64) -> bool {
    if size < MIN_PROBE_BYTES {
        return false;
    }
    let mut buf = vec![0u8; 64 * 1024];
    let n = match std::fs::File::open(path).and_then(|mut f| {
        use std::io::Read;
        f.read(&mut buf)
    }) {
        Ok(n) => n,
        Err(_) => return false,
    };
    video_signature(&buf[..n]).is_some()
}

/// 解析结果。
///
/// 只负责「把文件名拆成结构化字段」，**不负责判断这是电影还是剧集**——
/// 判定规则集中在 `mediatype` 模块（多信号打分，见那里的注释）。
///
/// `episode_marker` / `season_marker` 记录季集号是**从明确标记里读出来的**
/// 还是「补出来的默认值」：判定层要靠这个区分「真的看到了 S01E01」和
/// 「只看到个集号，季号是我自己补的 1」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedName {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub tmdb_id: Option<i64>,
    /// 集号来自明确标记（`S01E01` / `1x02` / `第01集` / `第01话` / `EP01` / `- 01`）。
    pub episode_marker: bool,
    /// 季号来自明确标记（`第2季` / `Season 2` / `S02`）。
    pub season_marker: bool,
}

/* ------------------------- 正则（惰性编译） ------------------------- */

// `lazy_re!` 在 `crate::util` 里只有一份定义，这里 import 即可。

lazy_re!(re_sxxexx, r"(?i)s(\d{1,2})[\s\-_]*e(\d{1,3})");
lazy_re!(re_1x02, r"(?i)\b(\d{1,2})x(\d{2,3})\b");
lazy_re!(re_ep_cn, r"第\s*(\d{1,3})\s*[集话話回]");
lazy_re!(re_season_cn, r"第\s*([0-9一二三四五六七八九十]{1,3})\s*季");
lazy_re!(re_season_en, r"(?i)\bseason[\s\-_]*(\d{1,2})\b");
// `Show.S02.1080p`：只写了季号、没写集号（`re_sxxexx` 认不出来）
lazy_re!(re_season_short, r"(?i)\bs(\d{1,2})\b");
lazy_re!(re_episode_word, r"(?i)\bepisodes?[\s\-_.]*(\d{1,3})\b");
lazy_re!(re_ep_en, r"(?i)\b(?:ep|e)[\s\-_.]*(\d{1,3})\b");
lazy_re!(re_year, r"\b(18\d{2}|19\d{2}|20\d{2})\b");
// TMDB 识别码：`tmdb-123456` / `tmdbid:123456` / `[tmdb=123456]` / `(tmdb 123456)`。
//
// 两条硬要求：
// 1. **必须有 `tmdb`（或 `tmdbid`）关键字**——否则 `2012`、`1080p` 这类
//    名称本身带数字的普通文本会被误当成 id；
// 2. 结尾的 `\b` 挡掉分辨率伪装：`tmdb 1080p` 里的 `1080` 后面紧跟 `p`，
//    没有词边界，不会被认成 id；
// 3. 位数锁 1–10 位：TMDB id 确实有 1–2 位的老片（`tmdb-7`），
//    但不会有 10 位以上，上限用来挡 `tmdb-12345678901234` 这种乱写。
lazy_re!(re_tmdb, r"(?i)\btmdb(?:\s*id)?[\s\-_=:.]*(\d{1,10})\b");
lazy_re!(re_dash_ep, r"[\-–—]\s*(\d{2,3})\s*$");
// `剧名.01.猴王初问世`：中文库最常见的「裸集号 + 集名」写法。
//
// 三条阀门，缺一条就会误伤电影：
// 1. 位数锁 2–3 位：躲开 4 位年份（`1986`）与 `XX 2` 这种单数字续作；
// 2. 后边界必须是 ASCII 分隔符，且**后面还得有内容**——`50度灰` `300勇士`
//    `12怒汉` 这类片名的数字后面紧跟汉字，落不到这条规则上；
// 3. 只认「集号后面还有集名」的形态。`剧名 01.mkv`（数字结尾）**不在这里认**，
//    留给判定层的「目录编号共识」处理——系列电影正是这个形状，
//    单个文件就判成剧集会把 `Rocky 1/2` 那类拆部电影误收进剧集。
lazy_re!(
    re_bare_ep,
    r"(?:^|[\s._\-–—\[\](){}]|[\x{4e00}-\x{9fff}])(\d{2,3})(?:[\s._\-–—\[\](){}])(\S)"
);
// `1986-2000版`：发行版本区间，不属于标题，取**起始年**。
lazy_re!(re_year_range, r"(\d{4})\s*[-–—~]\s*(\d{4})\s*(?:版本|年版|版)?");

// 注意：这里必须是**单行**字面量。Rust 原始字符串不支持行尾反斜杠续行，
// 换行会被当成模式的一部分，导致被折行的那几个标签永远匹配不上。
lazy_re!(
    re_ascii_tag,
    r"(?i)\b(2160p|1440p|1080p|720p|576p|480p|8k|4k|bluray|blu-ray|bdrip|brrip|bdmv|webrip|web-dl|webdl|web|hdtv|dvdrip|dvd|hdrip|remux|hdr10|hdr|dolbyvision|dv|x264|x265|h264|h265|hevc|avc|10bit|8bit|aac|ac3|eac3|dts|ddp|dd5|truehd|atmos|flac|mp3|opus|repack|proper|extended|unrated|imax|internal|multi|dual|chs|cht|eng|vostfr|subs|sub)\b"
);

/// CJK 标签（`\b` 对中文无效，用子串查找）。
const CJK_TAGS: &[&str] = &[
    "中字", "英字", "双字", "双语", "简体", "繁体", "简中", "繁中", "国语", "粤语", "国粤",
    "中英", "内嵌", "内封", "外挂", "字幕", "压制", "高清", "蓝光", "完整版", "未删减",
    "加长版", "收藏版", "特效", "无水印", "重制版", "合集",
];

/// 把 `.` `_` 等分隔符统一成空格，并压缩空白。
fn normalize_separators(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len());
    for ch in stem.chars() {
        match ch {
            '.' | '_' | '＋' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 去掉**所有**括号组（`[] () {} 【】 〔〕`）并压缩空白。
///
/// 关键点：`[字幕组]` 这类发行组前缀必须在标签识别**之前**去掉，否则其中的
/// “字幕”二字会被当成字幕语言标签，把标题整段截掉（曾经的线上 bug）。
/// 年份等数字信息另有来源（见 `parse_stem`），因此这里可以放心全部去除。
fn strip_all_groups(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '[' | '(' | '{' | '【' | '〔' => depth += 1,
            ']' | ')' | '}' | '】' | '〕' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 {
                    out.push(ch);
                }
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 找出版本标签在字符串中最早出现的字节下标。
fn first_tag_index(s: &str) -> Option<usize> {
    let mut best = re_ascii_tag().find(s).map(|m| m.start());
    for tag in CJK_TAGS {
        if let Some(i) = s.find(tag) {
            best = Some(best.map_or(i, |b| b.min(i)));
        }
    }
    best
}

/// 找出季/集/年份标记片段的起始下标（用于截断标题）。
fn first_se_index(s: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut consider = |i: usize| {
        best = Some(best.map_or(i, |b| b.min(i)));
    };
    for re in [
        re_sxxexx(),
        re_1x02(),
        re_ep_cn(),
        re_season_cn(),
        re_season_en(),
        re_season_short(),
        re_episode_word(),
        re_ep_en(),
        re_dash_ep(),
        re_tmdb(),
    ] {
        if let Some(m) = re.find(s) {
            consider(m.start());
        }
    }
    // 裸集号的前边界是**被匹配吃掉**的一个分隔符或汉字（`剧名01.集名`），
    // 用它当截断点会连剧名最后一个字一起切掉，这里只取数字本身的位置。
    if let Some(m) = re_bare_ep().captures(s).and_then(|c| c.get(1)) {
        consider(m.start());
    }
    best
}

fn parse_cn_number(s: &str) -> Option<i32> {
    if let Ok(n) = s.parse::<i32>() {
        return Some(n);
    }
    let mut total = 0i32;
    let mut cur = 0i32;
    for ch in s.chars() {
        let v = match ch {
            '一' => 1,
            '二' | '两' => 2,
            '三' => 3,
            '四' => 4,
            '五' => 5,
            '六' => 6,
            '七' => 7,
            '八' => 8,
            '九' => 9,
            '十' => {
                cur = if cur == 0 { 1 } else { cur };
                total += cur * 10;
                cur = 0;
                continue;
            }
            _ => return None,
        };
        cur = v;
    }
    Some(total + cur)
}

/// 清理标题：去括号残骸、压缩空白、修剪两端分隔符。
fn clean_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '[' | ']' | '{' | '}' | '【' | '】' | '〔' | '〕' => out.push(' '),
            _ => out.push(ch),
        }
    }
    let cleaned = out.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned
        .trim_matches(|c: char| c == '-' || c == '–' || c == '—' || c.is_whitespace())
        .trim()
        .to_string()
}

/// 解析文件名（可含扩展名），返回结构化结果。
/// 从任意文本里提取 TMDB 识别码。
///
/// 这是「文本 → TMDB id」的**唯一解析入口**：文件名、标题、目录名都走这里，
/// 规则只写一份，避免各处各写一条正则、彼此对不上。
///
/// 只认带 `tmdb`（或 `tmdbid`）关键字的写法，大小写与分隔符一律忽略：
/// `tmdb-123456` / `tmdbid:123456` / `[tmdb=123456]` / `(tmdb 123456)` /
/// `tmdb_123456` / `TMDB.123456` 都能命中。
/// **没有关键字的裸数字一律不算**，所以 `2012`、`1080p`、`第01集` 不会被误伤。
pub fn extract_tmdb_id(text: &str) -> Option<i64> {
    let norm = normalize_separators(text);
    re_tmdb()
        .captures(&norm)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<i64>().ok())
}

/// 按给定顺序依次在多个文本里找 TMDB 识别码，命中第一个就返回。
///
/// **顺序即优先级**：调用方把最可信的文本放前面
/// （通常是「原始文件名」优先于「清洗后的标题」）。
pub fn tmdb_id_from_texts(texts: &[&str]) -> Option<i64> {
    texts.iter().filter_map(|t| extract_tmdb_id(t)).next()
}

pub fn parse_file_name(file_name: &str) -> ParsedName {
    let stem = match file_name.rfind('.') {
        Some(i) if !file_name[i + 1..].contains(' ') => &file_name[..i],
        _ => file_name,
    };
    parse_stem(stem)
}

/// 解析不含扩展名的文件名主干。
pub fn parse_stem(stem: &str) -> ParsedName {
    let norm = normalize_separators(stem);
    let mut result = ParsedName::default();

    // 1) TMDB id：在**未去括号**的串上找，因为标记本身写在 `{tmdb-123}` 里。
    //    放在最前面——命中 id 就意味着身份已确定，后面的标题/年份/分辨率
    //    解析只是为了清洗展示用的标题，不影响匹配依据。
    result.tmdb_id = extract_tmdb_id(&norm);

    // 2) 去掉所有括号组，得到用于识别季/集与标题的干净串
    let det = strip_all_groups(&norm);

    // 3) 季 / 集
    if let Some(c) = re_sxxexx().captures(&det) {
        result.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
        result.season_marker = true;
        result.episode_marker = true;
    } else if let Some(c) = re_1x02().captures(&det) {
        result.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
        result.season_marker = true;
        result.episode_marker = true;
    } else if let Some(c) = re_ep_cn().captures(&det) {
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode_marker = true;
    } else if let Some(c) = re_episode_word().captures(&det) {
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode_marker = true;
    } else if let Some(c) = re_ep_en().captures(&det) {
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode_marker = true;
    } else if let Some(c) = re_dash_ep().captures(&det) {
        // `Show Name - 01`：无季信息，按第 1 季处理
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode_marker = true;
    } else if let Some(c) = re_bare_ep().captures(&det) {
        // `剧名.01.集名`：裸集号 + 集名，同样按第 1 季处理。
        // 放最后一条——任何**明确**标记（S01E01 / 第01集 / EP01 / -01）都优先。
        result.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.episode_marker = true;
    }
    if let Some(c) = re_season_cn().captures(&det) {
        result.season = c.get(1).and_then(|m| parse_cn_number(m.as_str()));
        result.season_marker = true;
    } else if let Some(c) = re_season_en().captures(&det) {
        result.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.season_marker = true;
    } else if let Some(c) = re_season_short().captures(&det) {
        // `Show.S02.1080p`：只有季号、没有集号（整季打包或季目录）
        result.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        result.season_marker = true;
    }
    if result.episode.is_some() && result.season.is_none() {
        result.season = Some(1); // 补出来的默认值，**不算明确标记**
    }

    // 4) 标题区域 = 第一个标签 / 第一个季集标记之前的部分
    let end = {
        let tag = first_tag_index(&det);
        let se = first_se_index(&det);
        match (tag, se) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => det.len(),
        }
    };
    let mut region = det[..end].to_string();

    // 5) 年份：优先取标题区域内最后一个年份（并把它从标题里摘掉）；
    //    区域里没有才回退到全串查找——回退时**不改动** region，
    //    避免用全串偏移去切割 region 造成越界/错切。
    let year_in = |s: &str| -> Option<(i32, std::ops::Range<usize>)> {
        let mut last: Option<(i32, std::ops::Range<usize>)> = None;
        for m in re_year().find_iter(s) {
            if let Ok(y) = m.as_str().parse::<i32>() {
                last = Some((y, m.range()));
            }
        }
        last
    };
    // 5a) 版本区间要**先于**单一年份处理：`西游记1986-2000版` 里的 `2000`
    //     不是上映年份，按 `year_in`「取最后一个年份」的规则会取成 2000。
    let mut year_done = false;
    if re_year_range().captures(&region).is_some() {
        if let Some(y) = re_year_range()
            .captures(&region)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<i32>().ok())
        {
            result.year = Some(y);
            year_done = true;
        }
        region = re_year_range().replace_all(&region, " ").to_string();
    }
    if !year_done {
        match year_in(&region) {
            Some((y, range)) => {
                result.year = Some(y);
                region.replace_range(range, " ");
            }
            None => {
                result.year = year_in(&norm).map(|(y, _)| y);
            }
        }
    }

    // 6) 兜底清理残留标记
    let mut region = re_sxxexx().replace_all(&region, " ").to_string();
    region = re_1x02().replace_all(&region, " ").to_string();
    region = re_ep_cn().replace_all(&region, " ").to_string();
    region = re_episode_word().replace_all(&region, " ").to_string();
    region = re_ep_en().replace_all(&region, " ").to_string();
    region = re_dash_ep().replace_all(&region, " ").to_string();
    // 注意：`re_bare_ep` **不进**清理列表——它匹配时会吃掉集名的第一个字符
    // （`(\S)` 是模式的一部分）。标题区域本来就已经截断在它之前，无需再清。
    region = re_season_cn().replace_all(&region, " ").to_string();
    region = re_season_en().replace_all(&region, " ").to_string();
    region = re_season_short().replace_all(&region, " ").to_string();

    let title = clean_title(&region);
    result.title = if title.is_empty() {
        clean_title(&det[..end.min(det.len())])
    } else {
        title
    };

    result
}

#[cfg(test)]
#[path = "tests/parser.rs"]
mod tests;
