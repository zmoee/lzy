//! 无限网盘（lzy）—— 单文件版。
//!
//! 一个可执行文件里装着：蓝奏云接口 SDK + 本地 HTTP 服务 + 内嵌的前端界面
//! + MCP 服务。两个入口：
//!
//! ```text
//! lzy          开窗口（Windows 上是无边框窗口）
//! lzy --mcp    跑 MCP 服务，给 AI 用
//! ```
//!
//! 另有一组给系统集成用的小开关（`--no-window` / `--upload` / `--install-menu`），
//! 不是给人手敲的命令行界面。

// Windows 上编成 GUI 子系统：双击不创建控制台窗口。
//
// 之所以能这么干：面向人的命令行已经去掉了。唯一还走标准流的是 MCP ——
// 而 MCP 客户端会自己给进程接管道，跟子系统是什么无关。
#![cfg_attr(windows, windows_subsystem = "windows")]

// 往 stderr 写一行，写不出去就算了。
//
// **不能用 println! / eprintln!** —— Windows GUI 子系统下从资源管理器启动时
// 没有控制台，标准句柄是空的，而 Rust 的 println! 写失败时会 panic，
// 那会让程序一启动就崩。这个宏必须定义在所有 mod 之前（macro_rules 按文本顺序生效）。
macro_rules! note {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

mod acw;
mod creds;
mod httpd;
mod lanzou;
mod mcp;
mod ops;
mod server;

#[cfg(windows)]
#[path = "window_win.rs"]
mod window;
#[cfg(not(windows))]
#[path = "window_other.rs"]
mod window;

#[cfg(windows)]
#[path = "notify_win.rs"]
mod notify;
#[cfg(not(windows))]
#[path = "notify_other.rs"]
mod notify;

#[cfg(windows)]
#[path = "menu_win.rs"]
mod menu;
#[cfg(not(windows))]
#[path = "menu_other.rs"]
mod menu;

use std::sync::Arc;
use std::time::Duration;

use server::Server;

const DEFAULT_PORT: u16 = 8790;
const DEFAULT_IDLE_SECS: u64 = 180;

const HELP: &str = "\
无限网盘（lzy）

  lzy                  打开窗口
  lzy --no-window      只跑本地 HTTP 服务，不打开窗口
  lzy --mcp            跑 MCP 服务（stdio），给 AI 调用
  lzy --upload [--to <id>] <文件...>
                       静默上传，做完弹提示框（资源管理器右键菜单用的就是这个）
  lzy --install-menu [--to <id>]   注册右键菜单「上传到蓝奏云备份」
  lzy --uninstall-menu             拆掉右键菜单

环境变量：LANZOU_PORT / LANZOU_IDLE_EXIT / LANZOU_USER / LANZOU_PWD
          LANZOU_PART_BYTES（调小分块，便于用小文件测分卷逻辑）";

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let args = &argv[1..];
    let has = |f: &str| args.iter().any(|a| a == f);

    if has("-h") || has("--help") {
        // 帮助走 stderr：stdout 只留给 MCP 的协议帧
        note!("{HELP}");
        return;
    }

    if has("--install-menu") {
        let folder_id = value_of(args, "--to").unwrap_or(lanzou::ROOT);
        match menu::install(folder_id) {
            Ok(cmd) => note!("已注册右键菜单「上传到蓝奏云备份」\n  命令: {cmd}"),
            Err(e) => note!("注册失败: {e}"),
        }
        return;
    }
    if has("--uninstall-menu") {
        match menu::uninstall() {
            Ok(true) => note!("已移除右键菜单项"),
            Ok(false) => note!("本来就没注册过，无事可做"),
            Err(e) => note!("移除失败: {e}"),
        }
        return;
    }

    let app = Server::new();

    // 账号从哪来（优先级从高到低）：
    //   1. 环境变量 —— 无头场景 / 临时换号用
    //   2. 本机加密存下来的凭据 —— 用户在界面里登录过一次
    //   3. 都没有 —— 界面会弹登录卡；MCP 那边会给明确提示
    if let (Ok(u), Ok(p)) = (
        std::env::var("LANZOU_USER"),
        std::env::var("LANZOU_PWD"),
    ) {
        if !u.is_empty() && !p.is_empty() {
            app.configure(creds::Credentials { user: u, pwd: p });
        }
    } else if let Some(c) = creds::load() {
        app.configure(c);
    }

    // MCP：纯 stdio，不起 HTTP 服务、不开窗口
    if has("--mcp") {
        std::process::exit(mcp::serve(app));
    }

    // 静默上传（资源管理器右键菜单调起）。没有终端，反馈靠提示框。
    if has("--upload") {
        let paths = files_after(args, "--upload");
        if paths.is_empty() {
            note!("--upload 后面要跟文件路径");
            std::process::exit(1);
        }
        let folder_id = value_of(args, "--to").unwrap_or(lanzou::ROOT);
        let res = ops::upload_paths(&app, &paths, folder_id);
        ops::notify_upload(&res);
        std::process::exit(if res.failed.is_empty() { 0 } else { 1 });
    }

    std::process::exit(run_app(app, has("--no-window") || has("-n")));
}

fn run_app(app: Arc<Server>, headless: bool) -> i32 {
    let port: u16 = std::env::var("LANZOU_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let url = format!("http://127.0.0.1:{port}/");

    // 已有实例在跑：只补开一个窗口，不再起第二份服务
    if is_port_busy(port) {
        if headless {
            note!("端口 {port} 已被占用，应该已经有一个实例在跑；换端口请设 LANZOU_PORT");
            return 1;
        }
        note!("[lzy] 已有实例在 {url} 运行，复用它");
        return window::open_only(&url);
    }

    let t0 = std::time::Instant::now();
    let http = match httpd::bind(port) {
        Ok(h) => h,
        Err(e) => {
            note!("[lzy] 监听 127.0.0.1:{port} 失败: {e}");
            return 1;
        }
    };
    note!(
        "[lzy] 服务已就绪: {url} （耗时 {}ms）",
        t0.elapsed().as_millis()
    );

    // 登录放后台：不挡窗口弹出，第一次 API 调用时也能兜住
    {
        let app = Arc::clone(&app);
        std::thread::spawn(move || match app.login() {
            Ok(uid) => note!("[lzy] 已登录, uid={uid}"),
            Err(e) => note!("[lzy] 启动登录失败（发起请求时会重试）: {e}"),
        });
    }

    let http_thread = {
        let app = Arc::clone(&app);
        std::thread::spawn(move || httpd::serve(http, app))
    };
    let _ = &http_thread;

    if headless {
        note!("[lzy] --no-window: 仅提供 API，按 Ctrl+C 退出");
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    // 窗口模式：Windows 上是无边框窗口，其它平台退化成开浏览器。
    // 窗口关掉后前端不再发心跳，闲置到点自己退出（只有浏览器那条路需要）。
    let idle = std::env::var("LANZOU_IDLE_EXIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_IDLE_SECS);
    window::run(Arc::clone(&app), &url, idle)
}

/// 取 `--to` 后面的值。
fn value_of(args: &[String], flag: &str) -> Option<i64> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1)?.parse().ok()
}

/// 取 `--upload` 后面的所有非选项参数（资源管理器会把选中项一股脑塞进来）。
fn files_after(args: &[String], flag: &str) -> Vec<String> {
    let i = match args.iter().position(|a| a == flag) {
        Some(i) => i,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut skip_next = false;
    for a in &args[i + 1..] {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--to" {
            skip_next = true;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        out.push(a.clone());
    }
    out
}

fn is_port_busy(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().expect("常量地址"),
        Duration::from_secs(1),
    )
    .is_ok()
}
