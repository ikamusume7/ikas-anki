// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 「方法名 → (service 索引, method 索引)」映射。
//!
//! 表由 `build.rs` 生成，来源与 rslib 内部的服务分发代码一致
//! （`anki_proto_gen::get_services`）。

use std::collections::HashMap;
use std::sync::LazyLock;

include!(concat!(env!("OUT_DIR"), "/method_table.rs"));

static TABLE: LazyLock<HashMap<&'static str, (u32, u32)>> = LazyLock::new(|| {
    METHOD_TABLE
        .iter()
        .map(|(name, service, method)| (*name, (*service, *method)))
        .collect()
});

/// 按 camelCase 方法名查找服务索引。
pub fn lookup(method: &str) -> Option<(u32, u32)> {
    TABLE.get(method).copied()
}
