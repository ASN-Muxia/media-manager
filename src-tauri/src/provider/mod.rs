//! `provider`：外部数据源与被它们喂养的服务。
//!
//! 数据源层次是固定的：**TMDb 权威 → TheTVDB 季/集结构 → Fanart.tv 美术
//! → OMDb 评分 → 大模型兜底**。大模型只影响「检索词」与「脏文件名的结构化」，
//! 绝不替代元数据 API。
//!
//! 共用的 HTTP 样板（`http`）在这里而不是放到 `core`：超时、鉴权错误文案、
//! hosts 自定义解析全都属于「怎么跟外部世界说话」，与领域规则无关。

pub mod douban;
pub mod fanart;
pub mod http;
pub mod llm;
pub mod omdb;
pub mod probe;
pub mod registry;
pub mod tmdb;
pub mod tvdb;
