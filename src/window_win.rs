//! Windows 无边框窗口。
//!
//! 用 tao 管窗口、wry 管 webview —— 直接用它俩，不引整个 Tauri：
//! 我不需要它的打包器和 IPC 框架，窗口只需要指向本机那个 HTTP 服务就行。
//!
//! 窗口是无边框的，所以标题栏、拖动、最小化/最大化/关闭全部由前端自己画。
//! 前端 `desktop.ts` 里本来就在找 `window.desktop` 这个桥，所以这里把它注入进去，
//! **前端一行都不用改**。拖动则是利用前端已有的 `.app-drag` / `.app-no-drag`
//! 两个 CSS 类，从注入的脚本里做事件委托 —— 同理，也是零改动。

use std::sync::Arc;
use std::time::Duration;

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{WebContext, WebViewBuilder};

use crate::server::Server;

/// 注入到页面里的桥。前端 `desktop.ts` 优先看 `window.desktop`，正好接上。
const BRIDGE_JS: &str = r#"
(function () {
  const send = (m) => { try { window.ipc.postMessage(m); } catch (_) {} };
  window.desktop = {
    minimize: () => send('min'),
    toggleMaximize: () => send('max'),
    close: () => send('close'),
    isMaximized: () => false,
  };
  // 拖动：前端用 .app-drag 标出可拖区域、.app-no-drag 标出其中的按钮。
  // 在 document 上做事件委托，所以脚本在 document-start 注入也能生效。
  document.addEventListener('mousedown', (e) => {
    const el = e.target && e.target.closest && e.target.closest('.app-drag');
    if (!el) return;
    if (e.target.closest && e.target.closest('.app-no-drag')) return;
    send('drag');
  });
})();
"#;

pub fn open_only(url: &str) -> i32 {
    open(url);
    0
}

pub fn run(app: Arc<Server>, url: &str, idle_secs: u64) -> i32 {
    let event_loop = EventLoopBuilder::new().build();

    // tao 的 Window 不是 Clone，但它 Send+Sync，用 Arc 共享给 IPC 回调
    let window = match WindowBuilder::new()
        .with_title("无限网盘")
        .with_inner_size(LogicalSize::new(1180.0, 760.0))
        .with_min_inner_size(LogicalSize::new(900.0, 560.0))
        // 无边框：标题栏由前端自己画
        .with_decorations(false)
        .with_resizable(true)
        .build(&event_loop)
    {
        Ok(w) => Arc::new(w),
        Err(e) => {
            note!("[lzy] 建窗口失败（{e}），退回浏览器");
            open(url);
            return idle_loop(app, idle_secs);
        }
    };

    let win = Arc::clone(&window);
    // WebView2 默认把它的配置目录（缓存、Cookie、Local Storage）建在
    // **exe 旁边**，名字叫 <exe名>.WebView2 —— 那样 exe 所在目录会被弄脏，
    // 看起来就不像"单文件"了。这里把它挪到标准的用户目录下。
    let mut web_ctx = WebContext::new(Some(webview_data_dir()));

    let _webview = match WebViewBuilder::with_web_context(&mut web_ctx)
        .with_url(url)
        .with_initialization_script(BRIDGE_JS)
        .with_ipc_handler(move |msg| {
            let body = msg.body();
            let win = Arc::clone(&win);
            match body.as_str() {
                "min" => win.set_minimized(true),
                "max" => win.set_maximized(!win.is_maximized()),
                "close" => std::process::exit(0),
                "drag" => {
                    let _ = win.drag_window();
                }
                _ => {}
            }
        })
        .build(&window)
    {
        Ok(w) => w,
        Err(e) => {
            note!("[lzy] 建 webview 失败（{e}），退回浏览器");
            open(url);
            return idle_loop(app, idle_secs);
        }
    };

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

/// WebView2 的配置目录放到 %LOCALAPPDATA%\lzy\webview，不要挨着 exe。
fn webview_data_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    let base = std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(crate::lanzou::cookie::data_base);
    let dir = base.join("lzy").join("webview");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn open(url: &str) {
    note!("[lzy] 请在浏览器打开: {url}");
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
}

fn idle_loop(app: Arc<Server>, idle_secs: u64) -> i32 {
    let limit = idle_secs.max(1) as u128 * 1000;
    loop {
        std::thread::sleep(Duration::from_secs(5));
        if app.idle_millis() as u128 > limit {
            return 0;
        }
    }
}
