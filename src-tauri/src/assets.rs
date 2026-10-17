//! 运行时前端资源：**优先读磁盘上的 `dist`，读不到再回退编译期嵌入的那份**。
//!
//! 存在的理由（已批准的架构改动）：前端产物原本只能靠编译期嵌进 exe，build.rs 里还
//! 有一条 `rerun-if-changed=../dist`——本意是“改了前端要重编”，代价却是每改一行
//! CSS/TS 都要重编 media-manager crate 并重新链接 17MB 的 exe（实测 41.6 秒），而启动
//! 器又是隐藏窗口跑的，用户看到的就是“点了没反应”。
//!
//! 现在把资源来源换成“运行时优先”：`tauri::generate_context!()` 生成的那份嵌入资源
//! 被包在这一层里当兜底，`get` 先查磁盘、查不到再问它。于是：
//!   * 开发机：`vite build`（亚秒）后重开应用即生效，**不再需要 cargo build**；
//!   * 分发机：编译期记下的绝对路径根本不存在，必然走回退分支，行为与改动前一致
//!     （分发包本就自带资源，不依赖外部 dist）。
//!
//! 磁盘目录由 build.rs 用 `cargo:rustc-env=MM_DIST_DIR=...` 在**编译期**写死为绝对
//! 路径，运行时只读这个常量。刻意不用“相对 exe 推 ../..”：exe 在 target/<profile>/ 下，
//! 层级随构建方式变化，打包后必然算错。

use std::borrow::Cow;
use std::path::PathBuf;

use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{Assets, Context, Runtime};

/// 编译期记下的 dist 绝对路径；没有这个环境变量时一律走嵌入回退。
///
/// 用 `option_env!` 而非 `env!`：不经 build.rs 手工编译时不该直接编译失败。
const DISK_DIST: Option<&str> = option_env!("MM_DIST_DIR");

/// 运行时资源提供者：磁盘 dist 优先，嵌入资源兜底。
pub struct RuntimeAssets<R: Runtime> {
    /// `generate_context!` 编译期嵌入的资源；磁盘查不到时用它。
    ///
    /// `Option` 只是为了让 [`RuntimeAssets::install`] 能用“先占位换出、再正式装上”
    /// 的两步接线（见该方法注释）；正常运行期它一定是 `Some`。
    embedded: Option<Box<dyn Assets<R>>>,
    /// 磁盘 dist 根目录；`None` 表示不做磁盘查找（编译期没拿到路径）。
    disk: Option<PathBuf>,
}

impl<R: Runtime> RuntimeAssets<R> {
    /// 接线：把 `Context` 里编译期嵌入的资源取出来当兜底，换成“磁盘优先”的实现。
    ///
    /// 为什么用 `set_assets` 而不是直接写 `context.assets = ...`：`Context::assets`
    /// 虽是 `pub` 字段，但 `Box<dyn Assets<R>>` 无法**整体移出**（移动会破坏结构体），
    /// 而 `set_assets` 是官方提供的对象替换接口，并且**会把旧值还回来**——正好就是
    /// 我们需要的那份 `EmbeddedAssets`，一步拿到手，不必绕 `mem::replace` + `unwrap`。
    pub fn install(mut context: Context<R>) -> Context<R> {
        // 第一步：装一个空壳，把真正的嵌入资源换出来（返回值即 `generate_context!` 的产物）。
        let embedded = context.set_assets(Box::new(Self::vacant()));
        // 第二步：装上正式实现，把嵌入资源作为兜底持有。
        context.set_assets(Box::new(Self {
            embedded: Some(embedded),
            disk: disk_dir(),
        }));
        context
    }

    /// 没有嵌入资源的空壳，只在 `install` 的两步之间短暂存在。
    fn vacant() -> Self {
        Self {
            embedded: None,
            disk: disk_dir(),
        }
    }

    /// 在磁盘 dist 里读一个 key；命中返回内容，未命中/读失败返回 `None`（转交嵌入回退）。
    fn read_disk(&self, key: &str) -> Option<Vec<u8>> {
        let root = self.disk.as_deref()?;
        let rel = safe_relative(key)?;
        std::fs::read(root.join(rel)).ok()
    }
}

/// 编译期写入的 dist 目录（空串等同于“没有”）。
fn disk_dir() -> Option<PathBuf> {
    DISK_DIST.filter(|p| !p.is_empty()).map(PathBuf::from)
}

impl<R: Runtime> Assets<R> for RuntimeAssets<R> {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        if let Some(bytes) = self.read_disk(key.as_ref()) {
            return Some(Cow::Owned(bytes));
        }
        // 磁盘没有（本机还没跑过 vite build / 分发包没有 dist / 文件读不动）→ 回退嵌入。
        self.embedded.as_ref()?.get(key)
    }

    /// 直接委托嵌入资源：本项目没有任何地方依赖“遍历运行时 dist”。
    ///
    /// 空壳状态下返回空迭代器而不是 panic——`install` 完成后不可能走到这里。
    fn iter(&self) -> Box<AssetsIter<'_>> {
        match self.embedded.as_ref() {
            Some(embedded) => embedded.iter(),
            None => Box::new(std::iter::empty::<(Cow<'_, str>, Cow<'_, [u8]>)>()),
        }
    }

    /// 同样委托嵌入那份：tauri.conf.json 的 csp 是 null，这里自己重算只会引入
    /// “算错 → CSP 变严 → 页面崩”的风险；嵌入资源的哈希是构建期算好的，语义一致。
    fn csp_hashes(&self, html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        match self.embedded.as_ref() {
            Some(embedded) => embedded.csp_hashes(html_path),
            None => Box::new(std::iter::empty()),
        }
    }
}

/// `AssetKey` → dist 之下的**安全相对路径**；任何可疑的 key 一律返回 `None`。
///
/// 这是本地文件服务器：放行就等于任意文件读取，所以按“宁可回退嵌入，也不碰磁盘”的
/// 口径做白名单式拼接：
///   * `..` —— 唯一能把拼接结果送出 dist 之外的东西，直接拒绝；
///   * 反斜杠 / `:` —— Windows 上反斜杠同样是分隔符；`:` 还能写出盘符（`C:`）或
///     NTFS 备用数据流（`index.html:stream`），一并拒绝；
///   * `.` 与空段 —— 就地丢弃（`AssetKey` 的路径一律以 `/` 开头，前导空段是“根”，
///     不是“上一级”）。
///
/// 拒绝 `..` 之后，`root.join(rel)` 不可能越出 root，因此不需要再做 canonicalize 校验
/// （那要求路径已存在，会给“读不到就回退”的正常分支添麻烦）。
fn safe_relative(key: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for segment in key.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains('\\') || segment.contains(':') {
            return None;
        }
        out.push(segment);
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 普通资源路径照常映射（前导 `/` 是 AssetKey 的根，不是磁盘根）。
    #[test]
    fn normal_keys_map_under_dist() {
        assert_eq!(
            safe_relative("/index.html"),
            Some(PathBuf::from("index.html"))
        );
        assert_eq!(
            safe_relative("/assets/index-abc.js"),
            Some(PathBuf::from("assets").join("index-abc.js"))
        );
        // AssetKey 会把分隔符统一成 `/`，重复分隔符不该变成“上一级”。
        assert_eq!(
            safe_relative("/assets//index.js"),
            Some(PathBuf::from("assets").join("index.js"))
        );
    }

    /// 路径穿越必须被拦住——这是本地文件服务器的底线。
    #[test]
    fn parent_dir_is_rejected() {
        assert_eq!(safe_relative("/../secret.txt"), None);
        assert_eq!(safe_relative("/assets/../../secret.txt"), None);
        assert_eq!(safe_relative("/.."), None);
    }

    /// Windows 专属逃逸口：反斜杠分隔符、盘符、NTFS 备用数据流。
    #[test]
    fn windows_escape_hatches_are_rejected() {
        assert_eq!(safe_relative("/..\\secret.txt"), None);
        assert_eq!(safe_relative("/C:/Windows/win.ini"), None);
        assert_eq!(safe_relative("/index.html:stream"), None);
    }

    /// 空 key / 只有根：没有可读的东西，交给嵌入回退。
    #[test]
    fn empty_keys_are_rejected() {
        assert_eq!(safe_relative(""), None);
        assert_eq!(safe_relative("/"), None);
        assert_eq!(safe_relative("/./"), None);
    }
}
