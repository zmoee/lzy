//! Windows 的完成提示：调 user32.dll 弹一个系统消息框。
//!
//! user32.dll 是 Windows 自带的系统库，不算第三方依赖 —— 整个程序
//! 仍然只有一个 exe。mingw 那边有 libuser32.a，所以直接 `#[link]` 就行，
//! 不需要引 `windows` crate。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, utype: u32)
        -> i32;
}

const MB_OK: u32 = 0x0000_0000;
const MB_ICONERROR: u32 = 0x0000_0010;
const MB_ICONINFO: u32 = 0x0000_0040;
const MB_SETFOREGROUND: u32 = 0x0001_0000;
const MB_TOPMOST: u32 = 0x0004_0000;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

pub fn show(title: &str, msg: &str, is_err: bool) {
    let icon = if is_err { MB_ICONERROR } else { MB_ICONINFO };
    let flags = MB_OK | MB_SETFOREGROUND | MB_TOPMOST | icon;
    let t = wide(title);
    let m = wide(msg);
    // 第一个参数 0 = 没有父窗口；这是右键菜单调起的孤儿进程，正常
    unsafe {
        MessageBoxW(ptr::null_mut(), m.as_ptr(), t.as_ptr(), flags);
    }
}
