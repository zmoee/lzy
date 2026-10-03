//! 上传 / 下载 / 提示这些动作。
//!
//! MCP 工具和右键菜单（`--upload`）都调这里，只有一份实现。

use std::fs;
use std::io::{self, Write};

use serde::Serialize;

use crate::lanzou;
use crate::server::{Server, UploadResponse};

#[derive(Serialize)]
pub struct UploadFailure {
    pub path: String,
    pub error: String,
}

#[derive(Serialize)]
pub struct UploadResult {
    pub uploaded: Vec<UploadResponse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<UploadFailure>,
}

/// 逐个上传本地文件。一个失败不影响后面的。
pub fn upload_paths(app: &Server, paths: &[String], folder_id: i64) -> UploadResult {
    let mut res = UploadResult {
        uploaded: Vec::new(),
        failed: Vec::new(),
    };
    for path in paths {
        if paths.len() > 1 {
            note!(
                "上传 {} ...",
                std::path::Path::new(path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone())
            );
        }
        match upload_one(app, path, folder_id) {
            Ok(up) => res.uploaded.push(up),
            Err(e) => res.failed.push(UploadFailure {
                path: path.clone(),
                error: e,
            }),
        }
    }
    res
}

fn upload_one(app: &Server, path: &str, folder_id: i64) -> Result<UploadResponse, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err("不支持上传目录，请先打包成压缩包".into());
    }
    let f = fs::File::open(path).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".into());
    app.upload(f, &name, folder_id, meta.len() as i64)
        .map_err(|e| e.to_string())
}

/// 下载一个文件；`folder` 为真时把一个分卷文件夹合并还原。返回字节数。
pub fn download(app: &Server, id: &str, dest: &str, folder: bool) -> Result<u64, String> {
    let mut out = fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut total: u64 = 0;

    if folder {
        let parts = app.part_files(id).map_err(|e| e.to_string())?;
        if parts.is_empty() {
            return Err("这个文件夹是空的".into());
        }
        for (i, part) in parts.iter().enumerate() {
            let n = fetch_part(app, &part.id, &mut out).map_err(|e| {
                format!("第 {}/{} 块（{}）失败: {e}", i + 1, parts.len(), part.name)
            })?;
            total += n;
            eprint!("\r  已下载 {}/{}, {}          ", i + 1, parts.len(), human(total));
            let _ = io::stderr().flush();
        }
        note!();
    } else {
        total = fetch_part(app, id, &mut out)?;
    }
    Ok(total)
}

fn fetch_part(app: &Server, file_id: &str, out: &mut dyn Write) -> Result<u64, String> {
    let url = app.direct_link(file_id).map_err(|e| e.to_string())?;
    let mut stream = lanzou::open_direct(&url).map_err(|e| e.to_string())?;
    if stream.status >= 400 {
        return Err(format!("上游返回 HTTP {}，直链可能已失效", stream.status));
    }
    if stream.content_type.to_lowercase().starts_with("text/html") {
        return Err("直链已失效或被拒绝（拿到的是 HTML 页面而不是文件）".into());
    }
    io::copy(&mut stream.body, out)
        .map_err(|e| e.to_string())
}

/// 右键菜单那条路传完要弹个框 —— 那个进程没有终端，不给反馈用户会以为没反应。
pub fn notify_upload(res: &UploadResult) {
    let ok = res.uploaded.len();
    let bad = res.failed.len();
    if bad == 0 && ok == 1 {
        let up = &res.uploaded[0];
        if up.parts > 1 {
            let f = up.folder.as_ref().expect("分卷必有 folder");
            crate::notify::show(
                "上传完成",
                &format!(
                    "{}\n\n文件较大，已切成 {} 块放进网盘的「{}」文件夹。",
                    f.name, up.parts, f.name
                ),
                false,
            );
        } else if let Some(f) = &up.file {
            crate::notify::show("上传完成", &format!("{}  ({})", f.name, f.size), false);
        }
    } else if bad == 0 {
        crate::notify::show("上传完成", &format!("共 {ok} 个文件已上传到网盘。"), false);
    } else {
        let lines: Vec<String> = res
            .failed
            .iter()
            .map(|f| {
                let base = std::path::Path::new(&f.path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| f.path.clone());
                format!("{base}: {}", f.error)
            })
            .collect();
        crate::notify::show(
            "上传有失败",
            &format!("成功 {ok} 个，失败 {bad} 个：\n\n{}", lines.join("\n")),
            true,
        );
    }
}

/// 把字节数写成人看的样子。
///
/// 别用"i 从 -1 开始"那种写法 —— 移植时踩过：`i as usize` 会把 -1 变成
/// usize::MAX，循环条件恒假，最后下标越界。
pub fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut i = 0usize;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}


#[cfg(test)]
mod tests {
    use super::human;

    #[test]
    fn human_formats_all_orders() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(49), "49 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1024), "1.0 K");
        assert_eq!(human(38297), "37.4 K");
        assert_eq!(human(104857600), "100.0 M");
        assert_eq!(human(262144000), "250.0 M");
        assert_eq!(human(1073741824), "1.0 G");
        assert_eq!(human(1099511627776), "1.0 T");
        assert_eq!(human(u64::MAX), "16777216.0 T");
    }
}
