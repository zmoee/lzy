//! 把 assets/icon.ico 编进 Windows exe 的资源段（也就是资源管理器里看到的那个图标）。
//!
//! 注意 build.rs 跑在**宿主**上（我这边是 Linux），所以判断目标平台要看
//! `CARGO_CFG_TARGET_OS`，不能用 `#[cfg(windows)]` —— 那判断的是宿主，
//! 交叉编译时会判断错。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    // 前端是**编译时**由 rust-embed 嵌进二进制的 —— 只跑 `pnpm build` 而不重编
    // 这个 crate，二进制里还是旧界面。这条让 cargo 盯着 web/dist，
    // 改了前端就会触发重新嵌入。
    println!("cargo:rerun-if-changed=web/dist");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return; // Linux / macOS 版没有资源段这回事
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ico = manifest.join("assets").join("icon.ico");
    if !ico.exists() {
        println!("cargo:warning=assets/icon.ico 不在，跳过图标。先跑: python3 assets/make-icon.py");
        return;
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR 应该由 cargo 提供"));

    // 写资源脚本。图标路径用绝对路径，免得依赖工作目录。
    let rc_path = out.join("lzy-icon.rc");
    std::fs::write(&rc_path, format!("1 ICON \"{}\"\n", ico.display()))
        .expect("写 .rc 失败");

    let res_path = out.join("lzy-icon.res");
    let out_call = Command::new("llvm-rc")
        .arg("/fo")
        .arg(&res_path)
        .arg(&rc_path)
        .output()
        .map_err(|e| format!("调 llvm-rc 失败（Ubuntu 上装 llvm 就有）: {e}"));

    let out_call = match out_call {
        Ok(o) => o,
        Err(e) => panic!("{e}"),
    };
    if !out_call.status.success() {
        panic!(
            "llvm-rc 编译图标失败:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out_call.stdout),
            String::from_utf8_lossy(&out_call.stderr)
        );
    }

    // .res 直接丢给链接器，会被并进 PE 的资源段
    println!("cargo:rustc-link-arg={}", res_path.display());
}
