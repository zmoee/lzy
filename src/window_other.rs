//! 非 Windows 平台的"窗口"。
//!
//! 无边框窗口要靠 WebView2，那是 Windows 专有的一套。其它平台这里退化成
//! 用系统浏览器打开（有地址栏，但功能一样），并靠"前端不再发心跳"来判断
//! 窗口已关、闲置退出。

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::server::Server;

pub fn open_only(url: &str) -> i32 {
    open_browser(url);
    0
}

pub fn run(app: Arc<Server>, url: &str, idle_secs: u64) -> i32 {
    open_browser(url);

    let limit = (idle_secs as i64) * 1000;
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let idle = app.idle_millis();
        if idle > limit {
            note!("[lzy] 闲置 {} 秒（窗口已关？），退出", idle / 1000);
            return 0;
        }
    }
}

fn open_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    match Command::new(cmd).arg(url).spawn() {
        Ok(_) => note!("[lzy] 已用系统浏览器打开 {url}"),
        Err(_) => note!("[lzy] 请在浏览器打开: {url}"),
    }
}
