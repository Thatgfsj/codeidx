//! Sample Rust fixture: library root.
//!
//! 验收关注符号：`unicorn_marker_fn`、`GoldConfig`、`GoldConfig::forge`、`forge_config`；
//! `helper_spinner` 在本文件中被引用 3 次，PageRank 权重应显著高于其它符号。

pub mod utils;

/// 配置载体。
pub struct GoldConfig {
    /// 展示用标签。
    pub label: String,
    /// 传递给 helper_spinner 的深度。
    pub depth: u32,
}

impl GoldConfig {
    /// 从种子标签锻造新配置。
    pub fn forge(label: &str) -> GoldConfig {
        GoldConfig {
            label: label.to_owned(),
            depth: 2,
        }
    }
}

/// 验收标记函数：内部三次引用 utils::helper_spinner。
pub fn unicorn_marker_fn(cfg: &GoldConfig) -> String {
    let a = utils::helper_spinner(cfg.depth);
    let b = utils::helper_spinner(cfg.depth + 1);
    let c = utils::helper_spinner(cfg.depth + 2);
    format!("{a}-{b}-{c}")
}

/// 为 main 提供的便捷包装。
pub fn forge_config(label: &str) -> GoldConfig {
    GoldConfig::forge(label)
}
