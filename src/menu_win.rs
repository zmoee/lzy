//! 把 lzy 挂到资源管理器的右键菜单上（Windows 专属）。
//!
//! 走静态动词注册 —— 往 HKCU 写几个键，**不需要管理员权限**，
//! 也不装任何东西：没有 COM 组件、没有壳扩展 DLL、没有服务。
//! 卸载就是把这几个键删掉。
//!
//! 已知限制：Windows 11 从 22H2 起把这类菜单项收进了「显示更多选项」，
//! 所以要右键 → 显示更多选项 → 上传到蓝奏云。想进一级菜单得做 MSIX 稀疏包
//! 加 IExplorerCommand 壳扩展，那个要签名、要 COM，代价高得多。

use std::os::windows::process::CommandExt;
use std::process::Command;

use crate::lanzou::ROOT;

const MENU_KEY: &str = r"HKCU\Software\Classes\*\shell\LzyUpload";

/// 不让 reg.exe 弹控制台窗口。lzy 自己是 GUI 子系统，没有控制台可继承，
/// 不设这个标志的话每次装/卸都会闪一个黑框。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn install(folder_id: i64) -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("取不到自身路径: {e}"))?
        .canonicalize()
        .map_err(|e| format!("解析自身路径失败: {e}"))?
        .display()
        .to_string();

    let to = if folder_id != ROOT {
        format!(" --to {folder_id}")
    } else {
        String::new()
    };
    // %* 是资源管理器传进来的全部选中项。
    // MultiSelectModel=Player 让多选时只起一个进程、全部文件一次传完 ——
    // 默认模型是每个文件起一个进程，选十个文件就是十次登录同时打过去。
    // 命令行界面已经去掉了，右键菜单走 --upload 这个开关
    let cmdline = format!("\"{exe}\" --upload{to} %*");

    let steps: Vec<Vec<String>> = vec![
        vec!["add".into(), MENU_KEY.into(), "/ve".into(), "/d".into(), "上传到蓝奏云备份".into(), "/f".into()],
        vec!["add".into(), MENU_KEY.into(), "/v".into(), "Icon".into(), "/d".into(), format!("{exe},0"), "/f".into()],
        vec!["add".into(), MENU_KEY.into(), "/v".into(), "MultiSelectModel".into(), "/d".into(), "Player".into(), "/f".into()],
        vec!["add".into(), format!(r"{MENU_KEY}\command"), "/ve".into(), "/d".into(), cmdline.clone(), "/f".into()],
    ];

    for args in steps {
        let out = Command::new("reg")
            .creation_flags(CREATE_NO_WINDOW)
            .args(&args)
            .output()
            .map_err(|e| format!("调 reg.exe 失败: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "注册表写入失败（{}）: {}",
                args.get(1).cloned().unwrap_or_default(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    Ok(cmdline)
}

pub fn uninstall() -> Result<bool, String> {
    let out = Command::new("reg")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["delete", MENU_KEY, "/f"])
        .output()
        .map_err(|e| format!("调 reg.exe 失败: {e}"))?;

    if out.status.success() {
        return Ok(true);
    }
    // 键本来就不存在时 reg 会报错，这不算失败
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("unable to find") || err.contains("找不到") {
        return Ok(false);
    }
    Err(format!("删除注册表项失败: {}", err.trim()))
}
