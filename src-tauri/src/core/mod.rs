//! `core`：纯领域逻辑，不依赖界面、不碰 Tauri 命令。
//!
//! 这一层的职责是「把一个文件路径变成结构化的元数据判断」：
//! 解析文件名 → 判定是电影还是剧集 → 把同目录的文件聚成剧集 → 入库。
//! 换句话说，即使把整个 GUI 和数据库换掉，这里也不用改一行。
//! 这也是本项目最容易写单测的一部分（175+ 条 Rust 单测大多在此）。

pub mod diff;
pub mod grouping;
pub mod matching;
pub mod mediatype;
pub mod nfo;
pub mod parser;
// 资源目录库的纯规则：路径指纹判定 + 何时把库里那行标成 missing。
pub mod registry;
pub mod scanner;
pub mod tree;
