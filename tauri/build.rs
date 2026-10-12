// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 生成「HTTP 方法名 → (service 索引, method 索引)」映射表。
//!
//! 这两个索引必须与 rslib 内部由 `rslib/rust_interface.rs` 生成的分发代码
//! （`Backend::run_service_method` 的 match 分支）完全一致。因此这里复用
//! 同一个 `anki_proto_gen::get_services()`，而不是自行编号 —— 这正是
//! `anki_proto_gen` 存在的意义（见其 crate 文档注释）。

use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use inflections::Inflect;
use prost_reflect::DescriptorPool;

fn main() -> Result<()> {
    tauri_build::build();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DESCRIPTORS_BIN");

    let descriptors = descriptors_path()?;
    println!("cargo:rerun-if-changed={}", descriptors.display());

    let bytes = fs::read(&descriptors).with_context(|| {
        format!(
            "无法读取 protobuf 描述符 {}\n\
             请先在本仓库根目录执行：tools/ninja rslib:proto",
            descriptors.display()
        )
    })?;

    let pool =
        DescriptorPool::decode(bytes.as_slice()).context("解析 protobuf 描述符失败")?;

    // 只关心 Backend 侧服务：Collection 方法会被合并进 Backend 服务，
    // 因此 backend_services 覆盖了全部可调用接口。
    let (_collection_services, backend_services) = anki_proto_gen::get_services(&pool);

    // 关键：与服务分发保持一致。
    //
    // `get_services` 会把 FrontendService 的方法复制进 BackendFrontendService
    // （作为 delegating_methods），而 `rslib/rust_interface.rs` 又把
    // BackendFrontendService 过滤掉了 —— 这些方法由 GUI 层实现。
    // 若不过滤，请求会命中此表并被转给 rslib，最终得到 InvalidServiceIndex，
    // 而不是落到本外壳的平台端点实现（crate::platform）。
    let backend_services = backend_services
        .into_iter()
        .filter(|service| service.name != "BackendFrontendService");

    let mut out = String::new();
    out.push_str("// 本文件由 build.rs 自动生成，请勿手动修改。\n");
    out.push_str("pub static METHOD_TABLE: &[(&str, u32, u32)] = &[\n");
    for service in backend_services {
        for method in service
            .trait_methods
            .iter()
            .chain(service.delegating_methods.iter())
        {
            // 前端（mediasrv 的 stringcase.camelcase）使用 camelCase 方法名
            let camel = method.name.to_camel_case();
            out.push_str(&format!(
                "    ({camel:?}, {}, {}),\n",
                service.index, method.index
            ));
        }
    }
    out.push_str("];\n");

    let dest = PathBuf::from(env::var("OUT_DIR")?).join("method_table.rs");
    fs::write(&dest, out)?;
    Ok(())
}

/// 与 `anki_proto_gen::descriptors_path()` 保持一致的查找顺序。
fn descriptors_path() -> Result<PathBuf> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    if let Ok(path) = env::var("DESCRIPTORS_BIN") {
        let path = PathBuf::from(path);
        return Ok(if path.is_absolute() {
            path
        } else {
            repo_root.join(path)
        });
    }
    Ok(repo_root.join("out/rslib/proto/descriptors.bin"))
}
