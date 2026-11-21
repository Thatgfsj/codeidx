//! Sample Rust fixture: shared utilities.

/// 被库根高频引用的自旋助手。
pub fn helper_spinner(n: u32) -> u32 {
    n.wrapping_mul(7).wrapping_add(1)
}
