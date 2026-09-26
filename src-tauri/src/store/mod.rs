//! `store`：SQLite 持久化。
//!
//! 约定写在这里以免走偏：
//! - 结构变更一律走 `ensure_column`（先 `PRAGMA table_info` 再 `ALTER TABLE`），
//!   保证幂等、不依赖用户当前的库版本；
//! - 改动后必须抬 `SCHEMA_VERSION`，并且要在**真实的旧库**上跑一次迁移验证；
//! - 所有写入动作放在同一个事务里，避免半改状态。

pub mod db;
