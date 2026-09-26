//! 剧集识别与二次校正。
//!
//! ## 识别流程（两级）
//!
//! **L0 目录归集（先按文件夹）**
//!   同一「剧集目录」下的所有视频先归到同一个 Series。剧集目录 = 媒体根目录下的
//!   第一层目录名；文件直接躺在根目录时，退回用文件名解析出的标题归桶。
//!   目录是**最强**的信号——用户自己建目录就是为了分剧，所以它优先级最高。
//!
//! **L1 同目录拆分（编号冲突才拆）**
//!   一个目录里出现两份 `S01E01`，说明这个目录混了多部剧（或同一季的多个版本）。
//!   此时按「归一化标题」把目录拆开。
//!   没有冲突就**不拆**——「第01话」「EP01」「1x01」这些写法不同但归一后一样，
//!   拆了反而会把一部剧分成好几个实体。
//!
//! **L2 兄弟目录合并（证据充分才合）**
//!   跨目录合并必须**同时**满足以下全部条件，缺一不可：
//!     1. 归一化标题相同，或相似度 ≥ `MERGE_THRESHOLD`；
//!     2. 两边都不是空标题（归一化后为空说明目录名没有信息量）；
//!     3. **至少一边**带真实编号（有命名规律才敢判断是同一部剧）；
//!        没有编号的那一边只有在**标题完全一致**时才允许并入——
//!        特辑 / 特别篇常常没有编号，只凭相似度把它们接进来风险太大；
//!     4. 合并后季集编号**不重叠**（重叠 = 同一季的重复副本或不同版本，合并会丢集）；
//!     5. 年份只在**标题不完全一致**时才用来否决：标题一模一样却年份不同，
//!        多半是同一部剧的不同年份版本 / 特辑（重拍通常会连目录名一起改）；
//!     6. 只在**同一个媒体根目录下**的兄弟目录之间合并，跨媒体根目录不合。
//!
//! **L3 组内集号连续化**
//!   归好组之后，按季给每集分配**互不重复**的集号：解析出来的集号只要在本季
//!   内唯一就原样保留，撞号或压根没有号的按从小到大补空位。
//!   这样 `01.mkv`/`02.mkv` 是 1、2 集，两份都没编号的特辑是 1、2 集，
//!   而 `S01E05` 不会被硬掰成第 3 集。
//!
//! ## 防误判清单（上面的条件就是为了挡这些）
//!
//! - **跨文件夹误合并**：默认不合；只有编号、标题、年份三重证据都对上才合。
//! - **同一剧集被拆成多个实体**：`Show/Season 1` 与 `Show/Season 2` 这种按季分目录的
//!   写法，归一化会抹掉季号再比对，且两季集数天然不重叠，因此能正确并成一条。
//! - **重名剧集**：季集重叠，或「标题只是相似 + 年份冲突」→ 不合，各自成条，
//!   展示名自动带目录名区分。
//! - **两边都没有命名规律的目录**：都不带编号 → 不合（宁可保持原样，也不猜）。

use std::collections::{BTreeMap, BTreeSet};

/// 归一化标题后仍要抹掉的噪声词（季号、年份、版本标签、**单集类型词**）。
///
/// 单集类型词（`特辑` / `特别篇` / `OVA` / `SP` …）必须抹掉：它们描述的是
/// 这一集的性质，不是剧名。留着它们，`神秘博士 特辑` 就永远和 `神秘博士`
/// 对不上，一部剧会被特辑拆成两条。
///
/// 英文的这批（`special` / `specials` / `christmas` / `extras`）同理：
/// `Doctor.Who.2005.Christmas.Special.2015` 归一成 `doctor who christmas
/// special`，跟正剧 `doctor who` 的相似度只有 0.5，跨不过 `MERGE_THRESHOLD`。
const NOISE_TOKENS: &[&str] = &[
    "2160p", "1440p", "1080p", "720p", "576p", "480p", "8k", "4k", "bluray", "blu-ray",
    "bdrip", "brrip", "bdmv", "webrip", "web-dl", "webdl", "web", "hdtv", "dvdrip", "dvd",
    "hdrip", "remux", "hdr10", "hdr", "dolbyvision", "dv", "x264", "x265", "h264", "h265",
    "hevc", "avc", "10bit", "8bit", "aac", "ac3", "eac3", "dts", "ddp", "dd5", "truehd",
    "atmos", "flac", "mp3", "opus", "repack", "proper", "extended", "unrated", "imax",
    "internal", "multi", "dual", "chs", "cht", "eng", "vostfr", "subs", "sub", "complete",
    "season", "series", "ova", "oad", "sp", "合集", "完结", "高清", "蓝光", "国语", "粤语",
    "简中", "繁中", "中字", "双语", "字幕", "压制", "特辑", "特别篇", "番外篇", "番外",
    "特别版",
    // 英文单集类型词与附件名：`Sample` 是随片附带的样片，`Extras` 是花絮。
    "special", "specials", "christmas", "extras", "sample", "proof",
];

/// 一个 token 是不是噪声：直接命中，或去掉尾部数字后命中（`特辑2` → `特辑`）。
fn is_noise_token(token: &str) -> bool {
    if NOISE_TOKENS.contains(&token) {
        return true;
    }
    let trimmed = token.trim_end_matches(|c: char| c.is_ascii_digit());
    !trimmed.is_empty() && trimmed != token && NOISE_TOKENS.contains(&trimmed)
}

/// token 是不是「编号」（集号）。
///
/// 归一化时必须把集号抹掉：留着它，`哆啦A梦 1000` 就会有一千个各不相同的
/// 「归一化标题」，一部上千集的动画会被切成一堆碎条目。
fn is_number_token(token: &str) -> bool {
    if token.is_empty() || token.len() > 4 || !token.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    match token.parse::<i32>() {
        // 落在年份区间的四位数是年份不是集号（年份另有逻辑抹除）
        Ok(n) if n > 0 && !(1800..2100).contains(&n) => true,
        _ => false,
    }
}

/// 字符是否是「有意义」的字符（字母数字 / CJK / 假名 / 谚文）。
fn is_meaningful(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || ('\u{4e00}'..='\u{9fff}').contains(&c)
        || ('\u{3040}'..='\u{30ff}').contains(&c)
        || ('\u{ac00}'..='\u{d7af}').contains(&c)
}

/// 季 / 集标记在 `[i..]` 处命中时返回要跳过的字符数，否则返回 0。
///
/// 只在**词首**匹配（前一个字符不是字母数字），否则会把 `E3` 这类正常标题吃掉。
fn marker_len(chars: &[char], i: usize) -> usize {
    let rest: String = chars[i..].iter().collect();
    let lower = rest.to_ascii_lowercase();

    let digits_after = |from: usize, max: usize| -> usize {
        let d: String = rest[from..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if d.is_empty() || d.len() > max { 0 } else { d.len() }
    };

    // `s01` / `s1`
    if lower.starts_with('s') {
        let n = digits_after(1, 2);
        if n > 0 {
            return 1 + n;
        }
    }
    // `season 2` / `season2`
    if let Some(tail) = lower.strip_prefix("season") {
        let lead = tail.len() - tail.trim_start().len();
        let n = digits_after(6 + lead, 2);
        if n > 0 {
            return 6 + lead + n;
        }
    }
    // `ep01` / `episode 3`
    if let Some(tail) = lower.strip_prefix("episode") {
        let lead = tail.len() - tail.trim_start().len();
        let n = digits_after(7 + lead, 3);
        if n > 0 {
            return 7 + lead + n;
        }
    }
    if lower.starts_with("ep") && !lower.starts_with("episode") {
        let n = digits_after(2, 3);
        if n > 0 {
            return 2 + n;
        }
    }
    // `e01`
    if lower.starts_with('e') {
        let n = digits_after(1, 3);
        if n > 0 {
            return 1 + n;
        }
    }
    // `第2季` / `第01话` / `第二季`
    if let Some(tail) = rest.strip_prefix('第') {
        let body: String = tail.chars().take(3).collect();
        for (len, _) in body.char_indices() {
            let end = 1 + len + 1;
            if let Some(&c) = chars.get(i + end - 1) {
                if matches!(c, '季' | '集' | '话' | '話' | '回') {
                    return end;
                }
            }
        }
        // 单位在 body 末尾紧邻时（`第2季`：body 取到 "2季"，长度 2）
        if let Some(pos) = body.find(['季', '集', '话', '話', '回']) {
            if pos > 0 {
                return 1 + pos + 1;
            }
        }
    }
    0
}

/// 抹掉季 / 集标记（`s01` / `season 2` / `第2季` / `e01` / `第01话` …）。
fn strip_season_markers(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        let at_word_start = i == 0 || !is_meaningful(chars[i - 1]);
        if at_word_start {
            let skip = marker_len(&chars, i);
            if skip > 0 {
                out.push(' ');
                i += skip;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 标题归一化：小写 → 抹季号 → 抹年份 → 去噪声词 → 抹集号 → 只保留有意义
/// 字符 → 压空白。
///
/// 归一后相同的两个名字，才允许被判为「同一部剧」。
pub fn normalize_title(s: &str) -> String {
    let mut s = s.to_ascii_lowercase();
    s = strip_season_markers(&s);

    // 年份：`(2019)` / `2019` 都抹掉
    let mut no_year = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].is_ascii_digit() && chars.len() - i >= 4 {
            let four: String = chars[i..i + 4].iter().collect();
            if let Ok(y) = four.parse::<i32>() {
                if (1800..2100).contains(&y) {
                    no_year.push(' ');
                    i += 4;
                    continue;
                }
            }
        }
        no_year.push(chars[i]);
        i += 1;
    }
    s = no_year;

    let cleaned: String = s
        .chars()
        .map(|c| if is_meaningful(c) { c } else { ' ' })
        .collect();

    let mut tokens: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|t| !is_noise_token(t))
        .collect();

    // 抹掉尾部的集号：`哆啦A梦 1000` → `哆啦A梦`，`0001` → ``（纯编号文件名
    // 里没有标题信息，归一为空是正确的）。
    if tokens.last().copied().map(is_number_token).unwrap_or(false) {
        tokens.pop();
    }

    // 归一化后为空（比如目录名就叫 `Season 1`，季号被抹完后什么都不剩）是
    // **有意的**结果：这样的名字没有可比对的标题信息，一律不参与跨目录合并。
    tokens.join(" ")
}

/// 「显然不是作品名」的目录名：媒体库的分类层、光盘分片、纯结构目录。
///
/// 它们是文件夹树里的**容器**，不是作品本身——`D:\电视剧` 是分类层，
/// `Disc 1` / `CD2` 是载体编号，都不该被当成一部作品。
const NOT_A_WORK: &[&str] = &[
    "cd", "disc", "disk", "dvd", "bd", "part", "side", "vol", "volume", "tv", "movies",
    "movie", "video", "videos", "media", "anime", "new folder", "新建文件夹", "未命名",
    "电影", "电视剧", "剧集", "动画", "动漫", "番剧", "纪录片", "综艺", "短剧", "音乐",
    "视频", "媒体", "资源", "下载", "种子", "其它", "其他",
];

/// 这个目录名能不能代表**一部作品**（作品目录 / 剧集根目录）。
///
/// 用于「文件夹优先识别」：先从路径里挑出作品目录，再把它当聚合单位，
/// 而不是把散落的文件各自归类。
pub fn looks_like_work_name(dir: &str) -> bool {
    let n = normalize_title(dir);
    if n.is_empty() {
        return false; // `Season 1` / `S01` 这种纯结构名，归一后就没了
    }
    // `Disc 2` / `CD1` 去掉编号再比对
    let bare: String = n.chars().filter(|c| !c.is_ascii_digit()).collect();
    let bare = bare.trim();
    !bare.is_empty() && !NOT_A_WORK.contains(&bare)
}

/// 编辑距离（输入都很短，直接二维 DP）。
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 标题相似度（0.0 ~ 1.0）：取「编辑距离比率」和「词集合 Jaccard」的较大值。
///
/// 取较大值是有意的：`Breaking Bad` 与 `Breaking.Bad` 编辑距离很近，
/// 而 `The Show` 与 `Show The` 词序不同但词集合一样——两种都要认出来。
pub fn similarity(a: &str, b: &str) -> f64 {
    let na = normalize_title(a);
    let nb = normalize_title(b);
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    if na == nb {
        return 1.0;
    }

    let dist = levenshtein(&na, &nb);
    let longest = na.chars().count().max(nb.chars().count()).max(1);
    let ratio = 1.0 - (dist as f64 / longest as f64);

    let ta: BTreeSet<&str> = na.split_whitespace().collect();
    let tb: BTreeSet<&str> = nb.split_whitespace().collect();
    let inter = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count().max(1) as f64;
    let jaccard = inter / union;

    ratio.max(jaccard)
}

/// 合并阈值：低于这个相似度，只看名字不敢认定是同一部剧。
pub const MERGE_THRESHOLD: f64 = 0.9;

/// 一个待归组的文件（识别流程的输入）。
#[derive(Debug, Clone)]
pub struct GroupInput {
    /// 所属媒体根目录的下标（跨根目录不合并）。
    pub root: usize,
    /// 一级目录名；文件直接躺在根目录时为空串。
    pub folder: String,
    /// 文件所在目录名（`Specials` / 发行包目录…）。
    ///
    /// 也要参与判断：文件名被发行组前缀污染时（`trips-doctor.who.s07.chrimbo.mkv`），
    /// 真正说清「它属于哪部剧」的反而是目录名。
    pub parent: Option<String>,
    /// 文件名解析出的标题。
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// 季 / 集号是**真的解析出来的**（明确标记，或文件名尾部的编号），
    /// 而不是「没解析出来，补了个 1」的占位值。
    ///
    /// 这个区分很关键：一堆没编号的特辑如果都顶着「第 1 集」，光看 `episode`
    /// 会以为它们互相冲突，进而把一部剧拆开。
    pub explicit: bool,
}

impl GroupInput {
    fn signature(&self) -> (i32, i32) {
        (self.season.unwrap_or(1), self.episode.unwrap_or(0))
    }
}

/// 一个候选分组（L0/L1 之后的中间形态）。
#[derive(Debug)]
struct Bucket {
    root: usize,
    /// 归入本组的成员下标。
    members: Vec<usize>,
    /// 参与合并判断用的归一化标题（成员标题的众数）。
    normalized: String,
    /// 目录名（可能为空）。
    folder: String,
    /// 给用户看的分组名（保留原始大小写；`normalized` 只用于比对）。
    display: String,
    episodes: BTreeSet<(i32, i32)>,
    numbered: usize,
    years: BTreeSet<i32>,
    /// 来自同目录拆分时的子组序号（0 = 该目录的主组）。
    split_index: usize,
}

impl Bucket {
    fn has_numbering(&self) -> bool {
        self.numbered > 0
    }
}

/// 并查集：把「认定是同一部剧」的桶连起来。
struct Union {
    parent: Vec<usize>,
}

impl Union {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.parent[r] != r {
            r = self.parent[r];
        }
        let mut c = x;
        while self.parent[c] != r {
            let next = self.parent[c];
            self.parent[c] = r;
            c = next;
        }
        r
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            // 恒定挂到较小的根上，保证结果与输入顺序无关
            let (lo, hi) = (ra.min(rb), ra.max(rb));
            self.parent[hi] = lo;
        }
    }
}

/// 组内标题的众数（按归一化后的出现次数取最多；并列时取字典序最小，保证稳定）。
fn majority_normalized(inputs: &[GroupInput], members: &[usize]) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for &i in members {
        let n = normalize_title(&inputs[i].title);
        if !n.is_empty() {
            *counts.entry(n).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|(ka, va), (kb, vb)| va.cmp(vb).then(kb.cmp(ka).reverse()))
        .map(|(k, _)| k)
        .unwrap_or_default()
}

/// 组名的原始写法：优先用目录名（用户自己起的，最好认），
/// 没有目录时取成员解析标题的众数（保留原始大小写，只用于展示与组键）。
fn majority_display(inputs: &[GroupInput], members: &[usize], folder: &str) -> String {
    if !folder.trim().is_empty() {
        return folder.trim().to_string();
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for &i in members {
        let t = inputs[i].title.trim().to_string();
        if !t.is_empty() {
            *counts.entry(t).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|(ka, va), (kb, vb)| va.cmp(vb).then(kb.cmp(ka).reverse()))
        .map(|(k, _)| k)
        .unwrap_or_default()
}

/// 桶的归一化标题：取成员标题的众数；成员标题都没有信息量（纯编号文件名）
/// 时退回**目录名**——`哆啦A梦/0001.mp4` 的剧名只能从目录名来。
fn normalized_of(inputs: &[GroupInput], b: &Bucket) -> String {
    let by_title = majority_normalized(inputs, &b.members);
    if !by_title.is_empty() {
        return by_title;
    }
    normalize_title(&b.folder)
}

/// 组里解析出的年份集合（两边都有年份且不一致就不合并）。
fn collect_years(inputs: &[GroupInput], members: &[usize]) -> BTreeSet<i32> {
    members
        .iter()
        .filter_map(|&i| inputs[i].year)
        .collect::<BTreeSet<_>>()
}

/// 这一簇所在的**目录**是不是已经在说「我就是主组那部剧」。
///
/// 文件名可能被发行组前缀、缩写弄得面目全非，但它住的目录通常是干净的
/// （`Doctor.Who.2005.2012.Christmas.Special...\trips-doctor.who.s07.chrimbo.mkv`）。
/// 目录名与主组归一标题相似 → 认它属于主组。
fn belongs_to_parent(inputs: &[GroupInput], ms: &[usize], main: &str) -> bool {
    if main.is_empty() {
        return false;
    }
    ms.iter()
        .filter_map(|&i| inputs[i].parent.as_deref())
        .any(|p| {
            let n = normalize_title(&strip_release_tag(p));
            !n.is_empty() && similarity(&n, main) >= MERGE_THRESHOLD
        })
}

/// 目录名 / 发行包名里的**发行组尾巴**：`[...]` 后缀和 `-GROUP`。
///
/// 只在比对**目录名**时用（见 `belongs_to_parent`）：目录名几乎总是照搬发布包
/// 的名字（`...X264-TRiPS[rarbg]`），不去掉这两个尾巴，「住在同名的发行目录里」
/// 这条最强证据就用不上。它不会碰标题的归一化结果，所以 `X-Men` / `Spider-Man`
/// 这类名字该怎么比还怎么比。
fn strip_release_tag(name: &str) -> String {
    let no_bracket = name.split('[').next().unwrap_or(name).trim();
    match no_bracket.rsplit_once('-') {
        Some((head, tail))
            if !head.is_empty()
                && tail.len() <= 12
                && !tail.is_empty()
                && tail.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            head.to_string()
        }
        _ => no_bracket.to_string(),
    }
}

/// 单独成组所需的**带真实编号**成员数。
///
/// 少于这个数量的簇没有自己的命名体系（`0001.mp4` 系列、`S03` 季标记、
/// 或者压根没编号的特辑都算），切出去只会把一部剧拆散。
const MIN_SPLIT_MEMBERS: usize = 2;

/// 一个目录里是否混了两部作品：同一个 (季, 集) 上站着两个**互不相似**的标题。
///
/// 只看「编号撞车」是不够的——同一季的重复压制件也会撞车，但它们明明是同一部剧。
fn has_title_conflict(inputs: &[GroupInput], members: &[usize]) -> bool {
    let mut by_sig: BTreeMap<(i32, i32), Vec<String>> = BTreeMap::new();
    for &i in members {
        if inputs[i].explicit {
            by_sig
                .entry(inputs[i].signature())
                .or_default()
                .push(normalize_title(&inputs[i].title));
        }
    }
    by_sig.values().any(|titles| {
        let mut diffs = titles.iter().enumerate().flat_map(|(i, a)| {
            titles[i + 1..]
                .iter()
                .filter(move |b| a != *b && similarity(a, b) < MERGE_THRESHOLD)
        });
        diffs.next().is_some()
    })
}

/// 冲突时把成员按标题相似度聚成若干簇，并裁掉「撑不起一部剧」的小簇。
///
/// - 簇 = 与簇代表相似度 ≥ `MERGE_THRESHOLD` 的一批成员（代表就是簇里第一个）；
/// - 成员最多的簇是主组，它也是各种零散文件的归宿；
/// - 其余簇只有在**至少 `MIN_SPLIT_MEMBERS` 个带真实编号的成员**时才保留，
///   否则并入主组。
fn split_clusters(inputs: &[GroupInput], members: &[usize]) -> Vec<(String, Vec<usize>)> {
    let mut reps: Vec<String> = Vec::new();
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for &i in members {
        let n = normalize_title(&inputs[i].title);
        let hit = reps.iter().position(|r| {
            if r.is_empty() || n.is_empty() {
                r.is_empty() && n.is_empty()
            } else {
                similarity(r, &n) >= MERGE_THRESHOLD
            }
        });
        match hit {
            Some(k) => clusters[k].push(i),
            None => {
                reps.push(n);
                clusters.push(vec![i]);
            }
        }
    }

    let mut all: Vec<(String, Vec<usize>)> = reps.into_iter().zip(clusters).collect();
    // 人多的在前（它才是主角）；并列时取代表名较小者，保证结果稳定
    all.sort_by(|(ra, va), (rb, vb)| vb.len().cmp(&va.len()).then(ra.cmp(rb)));

    let mut kept: Vec<(String, Vec<usize>)> = Vec::new();
    let mut stray: Vec<usize> = Vec::new();
    for (rep, ms) in all {
        if kept.is_empty() {
            kept.push((rep, ms));
            continue;
        }
        let numbered = ms.iter().filter(|&&i| inputs[i].explicit).count();
        if numbered >= MIN_SPLIT_MEMBERS && !belongs_to_parent(inputs, &ms, &kept[0].0) {
            kept.push((rep, ms));
        } else {
            stray.extend(ms);
        }
    }
    if !stray.is_empty() {
        stray.sort_unstable();
        kept[0].1.extend(stray);
        kept[0].1.sort_unstable();
    }
    kept
}

/// 两个桶能不能合并成同一部剧（L2 的全部条件都在这里）。
fn can_merge(a: &Bucket, b: &Bucket) -> bool {
    if a.root != b.root {
        return false; // 跨媒体根目录：不合并
    }
    if a.normalized.is_empty() || b.normalized.is_empty() {
        return false; // 空标题没有信息量：不合并
    }
    if !a.has_numbering() && !b.has_numbering() {
        return false; // 两边都没有编号：纯猜，不猜
    }
    if similarity(&a.normalized, &b.normalized) < MERGE_THRESHOLD {
        return false;
    }
    // 有一边不带编号：只有**标题完全一致**才敢把它接进来。
    // 特辑 / 特别篇常常没有集号，光凭相似度接容易接错剧。
    if (!a.has_numbering() || !b.has_numbering()) && a.normalized != b.normalized {
        return false;
    }
    if !a.episodes.is_disjoint(&b.episodes) {
        return false; // 季集重叠：是同一季的重复副本 / 不同版本
    }
    // 年份只在**标题不完全一致**时才否决：标题一模一样却年份不同，多半是同一部
    // 剧的不同年份版本 / 特辑；标题只是相似（比如《The Office》美版/英版）时，
    // 年份冲突才是「这是两部剧」的证据。
    if a.normalized != b.normalized
        && !a.years.is_empty()
        && !b.years.is_empty()
        && a.years.is_disjoint(&b.years)
    {
        return false;
    }
    true
}

/// 归组结果：`assign[i]` = 第 i 个输入属于第几组（组号从 0 起，稳定排序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPlan {
    pub assign: Vec<usize>,
    /// 每组的规范化分组键（写库用），下标即组号。
    pub keys: Vec<String>,
    /// `episodes[i]` = 第 i 个输入最终采用的 (季, 集)。
    ///
    /// 组内按季连续编号：解析出来的集号只要在本季内唯一就原样保留，撞号的
    /// 和没有号的按从小到大补空位。这样同一部剧不会因为「两份特辑都叫第 1 集」
    /// 而在入库时互相覆盖。
    pub episodes: Vec<(i32, i32)>,
}

/// 两级归集。输入顺序不影响输出（组内/组间排序都是确定的）。
pub fn plan_groups(inputs: &[GroupInput]) -> GroupPlan {
    if inputs.is_empty() {
        return GroupPlan {
            assign: Vec::new(),
            keys: Vec::new(),
            episodes: Vec::new(),
        };
    }

    // --- L0：先按「根目录 + 一级目录」归集；没有目录的按归一化标题归桶 ---
    let mut order: Vec<(usize, String)> = Vec::new();
    let mut index_of: BTreeMap<(usize, String), usize> = BTreeMap::new();
    let mut buckets: Vec<Bucket> = Vec::new();

    for (i, it) in inputs.iter().enumerate() {
        // 文件直接躺在根目录时按**解析标题**归桶：这里用原始标题而不是归一化标题，
        // 否则一堆归一化后为空的名字会挤进同一个桶。
        let raw_key = if it.folder.trim().is_empty() {
            format!("\u{0}title\u{0}{}", it.title.trim().to_lowercase())
        } else {
            it.folder.clone()
        };
        let map_key = (it.root, raw_key.clone());
        let bi = match index_of.get(&map_key) {
            Some(&b) => b,
            None => {
                let b = buckets.len();
                index_of.insert(map_key, b);
                order.push((it.root, raw_key));
                buckets.push(Bucket {
                    root: it.root,
                    members: Vec::new(),
                    normalized: String::new(),
                    folder: it.folder.clone(),
                    display: String::new(),
                    episodes: BTreeSet::new(),
                    numbered: 0,
                    years: BTreeSet::new(),
                    split_index: 0,
                });
                b
            }
        };
        buckets[bi].members.push(i);
        if it.explicit {
            buckets[bi].numbered += 1;
            buckets[bi].episodes.insert(it.signature());
        }
    }

    // --- L1：同目录里混了两部剧 → 拆。标题 Darwin 化聚类，只保留成规模的簇 ---
    let mut final_buckets: Vec<Bucket> = Vec::new();
    for b in buckets {
        // 光看「编号撞车」判冲突太敏感：特别篇常常既没有集号、又带着 `S08`
        // 这种季标记，占位集号很容易跟正片的 `S08E01` 撞上——一撞就把整个目录
        // 按标题切碎，一部剧散成一地单条目。真正的证据是：同一个 (季, 集) 上
        // 站着两个**互不相似**的标题。
        let conflict = has_title_conflict(inputs, &b.members);
        if !conflict {
            let mut b = b;
            b.normalized = normalized_of(inputs, &b);
            b.display = majority_display(inputs, &b.members, &b.folder);
            b.years = collect_years(inputs, &b.members);
            final_buckets.push(b);
            continue;
        }
        // 有冲突：按标题聚类切开。只有成规模的簇才单独成组（见 `split_clusters`），
        // 孤零零的一两条仍归主组——它们多半是被命名方式拖走的某一集，不是另一部剧。
        let clusters = split_clusters(inputs, &b.members);
        for (idx, (rep, members)) in clusters.into_iter().enumerate() {
            let mut nb = Bucket {
                root: b.root,
                members,
                normalized: String::new(),
                folder: b.folder.clone(),
                display: String::new(),
                episodes: BTreeSet::new(),
                numbered: 0,
                years: BTreeSet::new(),
                split_index: idx,
            };
            for &i in &nb.members {
                if inputs[i].explicit {
                    nb.numbered += 1;
                    nb.episodes.insert(inputs[i].signature());
                }
            }
            nb.normalized = if rep.is_empty() {
                normalized_of(inputs, &nb)
            } else {
                rep
            };
            nb.display = majority_display(inputs, &nb.members, &b.folder);
            nb.years = collect_years(inputs, &nb.members);
            final_buckets.push(nb);
        }
    }

    // --- L2：兄弟目录合并（条件全在 can_merge 里）---
    let n = final_buckets.len();
    let mut uf = Union::new(n);
    for a in 0..n {
        for b in (a + 1)..n {
            if can_merge(&final_buckets[a], &final_buckets[b]) {
                uf.union(a, b);
            }
        }
    }

    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for b in 0..n {
        by_root.entry(uf.find(b)).or_default().push(b);
    }

    // 组号排序：成员多的在前，其次看最小成员下标——保证同一份输入永远同一份输出。
    let mut groups: Vec<Vec<usize>> = by_root.into_values().collect();
    for g in &mut groups {
        g.sort_unstable();
    }
    groups.sort_by(|a, b| {
        let pa = a.iter().map(|&i| final_buckets[i].members.len()).sum::<usize>();
        let pb = b.iter().map(|&i| final_buckets[i].members.len()).sum::<usize>();
        pb.cmp(&pa).then(a[0].cmp(&b[0]))
    });

    let mut assign = vec![0usize; inputs.len()];
    let mut keys = Vec::with_capacity(groups.len());
    let mut episodes = vec![(1i32, 0i32); inputs.len()];
    for (g, parts) in groups.iter().enumerate() {
        let members: Vec<usize> = parts
            .iter()
            .flat_map(|&b| final_buckets[b].members.iter().copied())
            .collect();
        for &b in parts {
            for &i in &final_buckets[b].members {
                assign[i] = g;
            }
        }
        // L3：组内集号连续化（撞号 / 缺号的补空位）
        for (i, se) in number_episodes(inputs, &members) {
            episodes[i] = se;
        }
        keys.push(canonical_key(&final_buckets, parts));
    }

    GroupPlan {
        assign,
        keys,
        episodes,
    }
}

/// 给一组内的每一集分配**互不重复**的集号（按季分别编号）。
///
/// - 真实编号在本季内唯一 → 原样保留（`S01E05` 不会被硬掰成第 3 集）；
/// - 撞号 / 没有号 → 从 1 开始找空位补上，顺序按「原集号 → 输入下标」，
///   保证同一份输入永远得到同一份结果。
fn number_episodes(inputs: &[GroupInput], members: &[usize]) -> Vec<(usize, (i32, i32))> {
    let mut by_season: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for &i in members {
        by_season
            .entry(inputs[i].season.unwrap_or(1))
            .or_default()
            .push(i);
    }

    let mut out = Vec::with_capacity(members.len());
    for (season, idxs) in by_season {
        // 统计每个真实编号被用了几次
        let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
        for &i in &idxs {
            if inputs[i].explicit {
                if let Some(e) = inputs[i].episode {
                    *counts.entry(e).or_insert(0) += 1;
                }
            }
        }
        let mut used: BTreeSet<i32> = BTreeSet::new();
        let mut assigned: BTreeMap<usize, i32> = BTreeMap::new();
        for &i in &idxs {
            if inputs[i].explicit {
                if let Some(e) = inputs[i].episode {
                    if counts.get(&e).copied().unwrap_or(0) == 1 {
                        assigned.insert(i, e);
                        used.insert(e);
                    }
                }
            }
        }
        // 剩下的按「原集号 → 输入下标」补空位
        let mut rest: Vec<usize> = idxs
            .iter()
            .copied()
            .filter(|i| !assigned.contains_key(i))
            .collect();
        rest.sort_by_key(|&i| (inputs[i].episode.unwrap_or(i32::MAX), i));
        let mut next = 1i32;
        for i in rest {
            while used.contains(&next) {
                next += 1;
            }
            assigned.insert(i, next);
            used.insert(next);
        }
        for i in idxs {
            out.push((i, (season, assigned.get(&i).copied().unwrap_or(0))));
        }
    }
    out
}

/// 组键：合并后取最短（并列取字典序最小）的目录名；没有目录时用归一化标题。
///
/// 键必须**可重复推导**，否则重扫一次就会新建重复剧集。
/// 同一目录被 L1 拆成多组时各组共用 base 会撞键，因此给非主组补上标题后缀。
fn canonical_key(buckets: &[Bucket], parts: &[usize]) -> String {
    let mut labels: BTreeSet<String> = BTreeSet::new();
    let mut split: Option<&Bucket> = None;
    for &b in parts {
        let bucket = &buckets[b];
        if !bucket.display.trim().is_empty() {
            labels.insert(bucket.display.trim().to_string());
        }
        if bucket.split_index > 0 && split.is_none() {
            split = Some(bucket);
        }
    }

    let shortest = |set: &BTreeSet<String>| {
        set.iter()
            .min_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)))
            .cloned()
    };

    let base = shortest(&labels).unwrap_or_else(|| "未命名".to_string());

    match split {
        Some(b) if !b.normalized.is_empty() => format!("{base} · {}", b.normalized),
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_ex(
        root: usize,
        folder: &str,
        title: &str,
        y: Option<i32>,
        s: Option<i32>,
        e: Option<i32>,
        explicit: bool,
    ) -> GroupInput {
        GroupInput {
            root,
            folder: folder.to_string(),
            parent: None,
            title: title.to_string(),
            year: y,
            season: s,
            episode: e,
            explicit,
        }
    }

    /// 带 parent（文件所在目录名）的输入。
    fn input_in(
        root: usize,
        folder: &str,
        parent: &str,
        title: &str,
        y: Option<i32>,
        s: Option<i32>,
        e: Option<i32>,
    ) -> GroupInput {
        let mut it = input(root, folder, title, y, s, e);
        it.parent = Some(parent.to_string());
        it
    }

    /// 带季 / 集号 → 视为「真的解析出了编号」。
    fn input(root: usize, folder: &str, title: &str, y: Option<i32>, s: Option<i32>, e: Option<i32>) -> GroupInput {
        input_ex(root, folder, title, y, s, e, s.is_some() || e.is_some())
    }

    fn groups_of(plan: &GroupPlan) -> usize {
        plan.keys.len()
    }

    fn episodes_of(plan: &GroupPlan) -> Vec<(i32, i32)> {
        plan.episodes.clone()
    }

    #[test]
    fn normalize_strips_season_year_and_noise() {
        assert_eq!(normalize_title("Breaking.Bad.S01.E01.1080p"), "breaking bad");
        assert_eq!(normalize_title("Show Season 2"), "show");
        assert_eq!(normalize_title("剧名 第2季"), "剧名");
        assert_eq!(normalize_title("Movie (2019)"), "movie");
        assert_eq!(normalize_title("The Show!"), "the show");
    }

    /// 目录名只有季号（`Season 1`）时归一为空 —— 有意的行为：
    /// 这类名字没有可比对信息，不该参与跨目录合并。
    #[test]
    fn pure_season_folder_normalizes_to_empty_on_purpose() {
        assert_eq!(normalize_title("Season 1"), "");
        assert_eq!(normalize_title("S01"), "");
    }

    #[test]
    fn similarity_is_one_for_same_title_different_punctuation() {
        assert_eq!(similarity("Breaking.Bad", "Breaking Bad"), 1.0);
        assert_eq!(similarity("The Show", "show the"), 1.0);
        assert!(similarity("Breaking Bad", "Better Call Saul") < MERGE_THRESHOLD);
    }

    #[test]
    fn same_folder_stays_one_group() {
        let inputs = vec![
            input(0, "Show", "Show", Some(2020), Some(1), Some(1)),
            input(0, "Show", "Show", Some(2020), Some(1), Some(2)),
            input(0, "Show", "Show", Some(2020), Some(2), Some(1)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 1);
        assert_eq!(plan.assign, vec![0, 0, 0]);
        assert_eq!(plan.keys, vec!["Show".to_string()]);
    }

    /// 跨一级目录：标题一致、编号不重叠 → 合并（避免一部剧被拆成多个实体）
    #[test]
    fn sibling_folders_with_disjoint_episodes_are_merged() {
        let inputs = vec![
            input(0, "Show Season 1", "Show", Some(2020), Some(1), Some(1)),
            input(0, "Show Season 1", "Show", Some(2020), Some(1), Some(2)),
            input(0, "Show Season 2", "Show", Some(2020), Some(2), Some(1)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
        assert_eq!(plan.assign, vec![0, 0, 0]);
    }

    /// 重名但季集重叠 → 绝不合并（防跨文件夹误合并的核心）
    #[test]
    fn same_title_with_overlapping_episodes_is_not_merged() {
        let inputs = vec![
            input(0, "Show A", "Show", Some(2020), Some(1), Some(1)),
            input(0, "Show B", "Show", Some(2020), Some(1), Some(1)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
    }

    /// 标题**完全一样**但年份不同：多半是同一部剧的不同年份版本 / 特辑
    /// （重拍通常会连目录名一起改），因此**要**合并。
    #[test]
    fn same_title_different_year_is_merged_when_titles_are_identical() {
        let inputs = vec![
            input(0, "Show (2008)", "Show", Some(2008), Some(1), Some(1)),
            input(0, "Show (2020)", "Show", Some(2020), Some(2), Some(1)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
    }

    /// 标题只是**相似**（不是完全一样）+ 年份冲突 → 不合并
    /// （《The Office》美版 / 英版就是这种形状）。
    #[test]
    fn similar_title_with_conflicting_year_is_not_merged() {
        let inputs = vec![
            input(0, "The Office US", "The Office US", Some(2005), Some(1), Some(1)),
            input(0, "The Office UK", "The Office UK", Some(2001), Some(2), Some(1)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
    }

    /// 跨媒体根目录：即使是同名同季集也不合并
    #[test]
    fn different_media_roots_are_never_merged() {
        let inputs = vec![
            input(0, "Show", "Show", Some(2020), Some(1), Some(1)),
            input(1, "Show", "Show", Some(2020), Some(2), Some(1)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 2);
    }

    /// 没有编号 → 只凭标题不敢合并
    #[test]
    fn folders_without_episode_numbering_are_not_merged() {
        let inputs = vec![
            input(0, "Show", "Show", Some(2020), None, None),
            input(0, "Show Extra", "Show", Some(2020), None, None),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
    }

    /// 一个目录里混了两部剧（编号冲突）→ 按标题拆开，且拆出的组键要互不相同
    #[test]
    fn conflicting_episode_numbers_inside_one_folder_split_the_group() {
        let inputs = vec![
            input(0, "Mix", "Alpha", Some(2020), Some(1), Some(1)),
            input(0, "Mix", "Alpha", Some(2020), Some(1), Some(2)),
            input(0, "Mix", "Beta", Some(2021), Some(1), Some(1)),
            input(0, "Mix", "Beta", Some(2021), Some(1), Some(2)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
        assert_ne!(plan.keys[0], plan.keys[1], "拆分后的组键不能撞车");
        // Alpha 两集一组、Beta 两集一组
        let mut per: Vec<usize> = vec![0; 2];
        for a in plan.assign {
            per[a] += 1;
        }
        assert_eq!(per, vec![2, 2]);
    }

    /// 没有冲突就不拆：「第01话」和「EP01」只是写法不同
    #[test]
    fn no_conflict_means_no_split() {
        let inputs = vec![
            input(0, "Anime", "Anime", None, Some(1), Some(1)),
            input(0, "Anime", "Anime", None, Some(1), Some(2)),
            input(0, "Anime", "Anime", None, Some(1), Some(3)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 1);
    }

    /// 文件直接躺在媒体根目录：按解析出的标题归桶
    #[test]
    fn loose_files_are_grouped_by_parsed_title() {
        let inputs = vec![
            input(0, "", "Breaking Bad", Some(2008), Some(1), Some(1)),
            input(0, "", "Breaking Bad", Some(2008), Some(1), Some(2)),
            input(0, "", "Dune", Some(2021), None, None),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
        assert_eq!(plan.assign[0], plan.assign[1]);
        assert_ne!(plan.assign[0], plan.assign[2]);
    }

    /// 结果与输入顺序无关（否则重扫一次分组就变了，会造出重复剧集）
    #[test]
    fn plan_is_independent_of_input_order() {
        let a = vec![
            input(0, "Show", "Show", Some(2020), Some(1), Some(1)),
            input(0, "Show Extra", "Show", Some(2020), Some(1), Some(2)),
        ];
        let b = vec![a[1].clone(), a[0].clone()];
        let pa = plan_groups(&a);
        let pb = plan_groups(&b);
        assert_eq!(pa.keys, pb.keys);
        assert_eq!(pa.assign[0], pb.assign[1]);
        assert_eq!(pa.assign[1], pb.assign[0]);
    }

    /// 组键要保留原始写法（用户看到的目录名），不能是归一化后的小写串。
    #[test]
    fn keys_keep_original_casing() {
        let inputs = vec![
            input(0, "Breaking Bad", "Breaking Bad", Some(2008), Some(1), Some(1)),
            input(0, "Breaking Bad", "Breaking Bad", Some(2008), Some(1), Some(2)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(plan.keys, vec!["Breaking Bad".to_string()]);
    }

    /* ---------------- 聚合回归（Phase F） ---------------- */

    /// 归一化必须抹掉**集号**：留着它，一部上千集的动画会被切成上千条。
    #[test]
    fn normalize_strips_trailing_episode_numbers() {
        assert_eq!(normalize_title("哆啦A梦 1000"), "哆啦a梦");
        assert_eq!(normalize_title("哆啦A梦 0001"), "哆啦a梦");
        assert_eq!(normalize_title("0001"), "", "纯编号文件名里没有标题信息");
        assert_eq!(normalize_title("1917"), "", "年份不是集号，但也不是标题");
        assert_eq!(normalize_title("Blade Runner 2049"), "blade runner");
    }

    /// 单集类型词（`特辑` / `特别篇` / `OVA`）不是剧名的一部分。
    #[test]
    fn normalize_strips_episode_type_words() {
        assert_eq!(normalize_title("神秘博士 特辑"), "神秘博士");
        assert_eq!(normalize_title("神秘博士 特辑2"), "神秘博士");
        assert_eq!(normalize_title("某剧 特别篇"), "某剧");
        assert_eq!(normalize_title("某动画 OVA"), "某动画");
    }

    /// 英文的单集类型词同样不是剧名的一部分（`Christmas Special` / `Extras`）。
    #[test]
    fn normalize_strips_english_episode_type_words() {
        assert_eq!(normalize_title("神秘博士 Christmas Special"), "神秘博士");
        assert_eq!(normalize_title("Doctor Who 2005 Christmas Special"), "doctor who");
        assert_eq!(normalize_title("doctor.who.2005.christmas.special.2005.1080p"), "doctor who");
        assert_eq!(normalize_title("Show - Special Edition"), "show edition");
        assert_eq!(normalize_title("Show 2015 Extras"), "show");
        assert_eq!(normalize_title("Show-Sample"), "show", "随片样片不是另一部剧");
    }

    /// 文件名被发行组前缀污染时，救回来的是**目录名**：
    /// `...X264-TRiPS[rarbg]\trips-doctor.who.s07.chrimbo.mkv` 虽然解析成
    /// `trips doctor who`，但它住的目录明明白白写着这是 Doctor Who 的发行包。
    #[test]
    fn polluted_filenames_are_pulled_back_by_their_directory() {
        let pkg = "Doctor.Who.2005.S07.1080p.BluRay.x264-MIXED[rartv]";
        let specials_pkg = "Doctor.Who.2005.2012.Christmas.Special.720p.BluRay.X264-TRiPS[rarbg]";
        let inputs = vec![
            input_in(0, "神秘博士", pkg, "doctor who", Some(2005), Some(7), Some(1)),
            input_in(0, "神秘博士", pkg, "doctor who", Some(2005), Some(7), Some(2)),
            input_in(0, "神秘博士", specials_pkg, "trips doctor who", Some(2012), Some(7), Some(1)),
            input_in(0, "神秘博士", specials_pkg, "trips doctor who", Some(2012), Some(7), Some(2)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
        let mut eps = episodes_of(&plan);
        eps.sort_unstable();
        assert_eq!(eps, vec![(7, 1), (7, 2), (7, 3), (7, 4)]);
    }

    /// 反过来：目录名没有指向主组时，混在一处的两部剧**仍然要**拆开。
    #[test]
    fn mixed_folder_still_splits_when_directory_says_nothing() {
        let inputs = vec![
            input_in(0, "Mix", "Mix", "Alpha", Some(2020), Some(1), Some(1)),
            input_in(0, "Mix", "Mix", "Alpha", Some(2020), Some(1), Some(2)),
            input_in(0, "Mix", "Mix", "Beta", Some(2021), Some(1), Some(1)),
            input_in(0, "Mix", "Mix", "Beta", Some(2021), Some(1), Some(2)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
    }

    /// 作品目录识别：分类层 / 结构目录不是作品，`Disc 2` 这类载体编号也不是。
    #[test]
    fn work_name_detection_skips_containers_and_categories() {
        for dir in ["神秘博士", "Breaking Bad", "哆啦A梦", "Legal High", "Doctor Who (2005)"] {
            assert!(looks_like_work_name(dir), "{dir} 应认作作品目录");
        }
        // 分类层目录。注意这里传的是**单层目录名**，不带路径——`work_dir_of`
        // 传进来的永远是路径的一段。
        for dir in ["Season 1", "S01", "电视剧", "电影", "动漫", "新建文件夹", "Movies", "TV"] {
            assert!(!looks_like_work_name(dir), "{dir} 不是作品目录");
        }
        // 载体 / 分片编号：去掉末尾数字后剩下的词在黑名单里
        for dir in ["Disc 1", "CD2", "DVD part 2", "Vol 3", "Vol.2"] {
            assert!(!looks_like_work_name(dir), "{dir} 不是作品目录");
        }
    }

    /// 没有编号的特辑也要并入同一部剧（标题完全一致是硬条件）。
    #[test]
    fn unnumbered_specials_join_the_numbered_series() {
        let inputs = vec![
            input(0, "神秘博士 (2005)", "神秘博士", Some(2005), Some(1), Some(1)),
            input(0, "神秘博士 (2005)", "神秘博士", Some(2005), Some(1), Some(2)),
            input_ex(0, "神秘博士 (2023)", "神秘博士 特辑", Some(2023), Some(0), Some(1), false),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
    }

    /// 两边都没编号 → 不合并（保持「不猜」的底线）。
    #[test]
    fn two_unnumbered_groups_are_still_not_merged() {
        let inputs = vec![
            input_ex(0, "神秘博士 (2023)", "神秘博士 特辑", Some(2023), Some(0), Some(1), false),
            input_ex(0, "神秘博士 (2024)", "神秘博士 特辑", Some(2024), Some(0), Some(1), false),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 2, "keys={:?}", plan.keys);
    }

    /// 纯编号文件名（`0001.mp4`）：靠目录名归一，不能被切成一堆碎条目。
    #[test]
    fn pure_number_files_in_one_folder_stay_one_group() {
        let inputs = vec![
            input(0, "哆啦A梦", "0001", None, Some(1), Some(1)),
            input(0, "哆啦A梦", "0002", None, Some(1), Some(2)),
            input(0, "哆啦A梦", "0003", None, Some(1), Some(3)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
        assert_eq!(plan.keys, vec!["哆啦A梦".to_string()]);
        assert_eq!(episodes_of(&plan), vec![(1, 1), (1, 2), (1, 3)]);
    }

    /// 集号连续化：撞号的补空位，而不是互相覆盖成一集。
    #[test]
    fn colliding_episode_numbers_are_renumbered_within_a_group() {
        let inputs = vec![
            input(0, "Show", "Show", None, Some(1), Some(1)),
            input(0, "Show", "Show", None, Some(1), Some(2)),
            // 两份都没有明确编号的特辑，占位值都是第 1 集
            input_ex(0, "Show", "Show 特辑", None, Some(0), Some(1), false),
            input_ex(0, "Show", "Show 特辑2", None, Some(0), Some(1), false),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(groups_of(&plan), 1, "keys={:?}", plan.keys);
        let mut eps = episodes_of(&plan);
        eps.sort_unstable();
        assert_eq!(
            eps,
            vec![(0, 1), (0, 2), (1, 1), (1, 2)],
            "特辑应补成第 1、2 集，而不是两集都叫第 1 集"
        );
    }

    /// 真实的稀疏集号要保留（`S01E05` 不能被硬掰成第 3 集）。
    #[test]
    fn unique_episode_numbers_are_kept_as_is() {
        let inputs = vec![
            input(0, "Show", "Show", None, Some(1), Some(1)),
            input(0, "Show", "Show", None, Some(1), Some(2)),
            input(0, "Show", "Show", None, Some(1), Some(5)),
        ];
        let plan = plan_groups(&inputs);
        assert_eq!(episodes_of(&plan), vec![(1, 1), (1, 2), (1, 5)]);
    }

    #[test]
    fn empty_input_gives_empty_plan() {
        let plan = plan_groups(&[]);
        assert!(plan.assign.is_empty() && plan.keys.is_empty());
    }
}

