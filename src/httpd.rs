//! HTTP 层：路由、JSON 响应、流式转发、内嵌前端。
//!
//! 用 tiny_http（同步、每请求一线程）。这个程序的负载是 I/O 阻塞型，
//! 不值得为它上 tokio。
//!
//! 接口契约与前端 `web/src/lib/api.ts` 一一对应。

use std::io::{self, Read};
use std::sync::Arc;
use std::thread;

use tiny_http::{Header, Method, Request, Response, Server as HttpServer, StatusCode};

use crate::lanzou::{self, LzError};
use crate::server::{error_body, rfc5987, PartRef, Server};

#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist"]
struct Assets;

pub fn bind(port: u16) -> io::Result<HttpServer> {
    HttpServer::http(("127.0.0.1", port)).map_err(|e| io::Error::other(e.to_string()))
}

/// 接受连接，每个请求一个线程。
pub fn serve(http: HttpServer, app: Arc<Server>) {
    for request in http.incoming_requests() {
        let app = Arc::clone(&app);
        thread::spawn(move || handle(app, request));
    }
}

fn handle(app: Arc<Server>, mut req: Request) {
    // 只认回环地址的 Host —— 挡住 DNS rebinding。
    // 攻击手法是让浏览器把 evil.com 解析到 127.0.0.1，然后以 evil.com 的身份
    // 调本地接口；那样 Host 头就是 evil.com，这里直接挡掉。
    // 没带 Host 的不是浏览器（命令行工具之类），放行。
    if let Some(host) = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Host"))
        .map(|h| h.value.as_str().to_string())
    {
        if !is_loopback_host(&host) {
            return fail_msg(req, 403, "拒绝：Host 头不是本机地址");
        }
    }

    let raw_url = req.url().to_string();
    let (path, query) = split_url(&raw_url);
    let method = req.method().clone();

    if path.starts_with("/api/") {
        app.touch();
    }

    match (method.clone(), path.as_str()) {
        // 前端启动时问一句"配过账号没有"，据此决定要不要弹登录卡。
        // 这个接口不需要已配置 —— 它就是用来问这件事的。
        (Method::Get, "/api/session") => {
            let user = app.current_user();
            json(
                req,
                200,
                &format!(
                    "{{\"configured\":{},\"user\":{}}}",
                    app.is_configured(),
                    crate::httpd::json_str(&user)
                ),
            )
        }

        // 界面登录：真去网盘登录一次验证，成功才记住。
        (Method::Post, "/api/login") => {
            let body = req
                .as_reader()
                .bytes()
                .take(64 * 1024)
                .collect::<Result<Vec<u8>, _>>()
                .unwrap_or_default();
            let v: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => return fail_msg(req, 400, &format!("请求格式不对: {e}")),
            };
            let user = v.get("user").and_then(|x| x.as_str()).unwrap_or("");
            let pwd = v.get("pwd").and_then(|x| x.as_str()).unwrap_or("");
            let remember = v.get("remember").and_then(|x| x.as_bool()).unwrap_or(true);
            match app.login_with(user, pwd, remember) {
                Ok(uid) => json(
                    req,
                    200,
                    &format!("{{\"uid\":{}}}", json_str(&uid)),
                ),
                Err(e) => fail(req, &e),
            }
        }

        // 退出登录。forget=true 时连存盘的凭据一起抹掉（换账号走这条）。
        (Method::Post, "/api/logout") => {
            let forget = q_str(&query, "forget").as_deref() == Some("true");
            app.logout(forget);
            json(req, 200, "{\"ok\":true}");
        }

        (Method::Get, "/api/ping") => json(req, 200, "{\"ok\":true}"),

        (Method::Get, "/api/status") => match app.status() {
            Ok(v) => json_value(req, 200, &v),
            Err(e) => fail(req, &e),
        },

        (Method::Get, "/api/list") => {
            let folder_id = q_i64(&query, "folder_id", lanzou::ROOT);
            let pg = q_i64(&query, "pg", 0);
            match app.list(folder_id, pg) {
                Ok(v) => json_value(req, 200, &v),
                Err(e) => fail(req, &e),
            }
        }

        (Method::Post, "/api/upload") => {
            let filename = q_str(&query, "filename").unwrap_or_else(|| "unnamed".into());
            let folder_id = q_i64(&query, "folder_id", lanzou::ROOT);
            let hint = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Content-Length"))
                .and_then(|h| h.value.as_str().parse::<i64>().ok())
                .unwrap_or(-1);
            let body = req.as_reader();
            match app.upload(body, &filename, folder_id, hint) {
                Ok(v) => json_value(req, 200, &v),
                Err(e) => fail(req, &e),
            }
        }

        // ---- 分卷上传（前端切块，一块一个请求）----

        (Method::Post, "/api/upload/begin") => {
            let v: serde_json::Value = match read_json(&mut req, 64 * 1024) {
                Ok(v) => v,
                Err(e) => return fail_msg(req, 400, &e),
            };
            let filename = v.get("filename").and_then(|x| x.as_str()).unwrap_or("");
            let folder_id = v.get("folder_id").and_then(|x| x.as_i64()).unwrap_or(lanzou::ROOT);
            let size = v.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
            if filename.is_empty() || size == 0 {
                return fail_msg(req, 400, "缺少 filename 或 size");
            }
            match app.upload_begin(filename, folder_id, size) {
                Ok(res) => json_value(req, 200, &res),
                Err(e) => fail(req, &e),
            }
        }

        (Method::Post, "/api/upload/part") => {
            let folder_id = q_str(&query, "folder_id").unwrap_or_default();
            let filename = q_str(&query, "filename").unwrap_or_default();
            let idx = q_i64(&query, "idx", 0);
            if folder_id.is_empty() || filename.is_empty() || idx < 1 {
                return fail_msg(req, 400, "缺少 folder_id / filename / idx");
            }
            // 单块也受服务端 100M 上限约束，多读 1 字节用来判断
            let data = match req
                .as_reader()
                .bytes()
                .take(crate::server::MAX_UPLOAD_BYTES as usize + 1)
                .collect::<Result<Vec<u8>, _>>()
            {
                Ok(d) => d,
                Err(e) => return fail_msg(req, 400, &format!("读取分块失败: {e}")),
            };
            if data.is_empty() {
                return fail_msg(req, 400, "空分块");
            }
            if data.len() as u64 > crate::server::MAX_UPLOAD_BYTES {
                return fail_msg(req, 413, "分块超过 100 M 上限");
            }
            match app.upload_part(&folder_id, idx as usize, &filename, data) {
                Ok(()) => json(req, 200, &format!("{{\"ok\":true,\"idx\":{idx}}}")),
                Err(e) => fail(req, &e),
            }
        }

        (Method::Post, "/api/upload/finish") => {
            let v: serde_json::Value = match read_json(&mut req, 64 * 1024) {
                Ok(v) => v,
                Err(e) => return fail_msg(req, 400, &e),
            };
            let folder_id = v.get("folder_id").and_then(|x| x.as_str()).unwrap_or("");
            let filename = v.get("filename").and_then(|x| x.as_str()).unwrap_or("");
            let parts = v.get("parts").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            if folder_id.is_empty() || filename.is_empty() || parts == 0 {
                return fail_msg(req, 400, "缺少 folder_id / filename / parts");
            }
            match app.upload_finish(folder_id, filename, parts) {
                Ok(res) => json_value(req, 200, &res),
                Err(e) => fail(req, &e),
            }
        }

        (Method::Post, "/api/relogin") => match app.relogin() {
            Ok(uid) => json(req, 200, &format!("{{\"uid\":{}}}", json_str(&uid))),
            Err(e) => fail(req, &e),
        },

        _ => {
            if let Some(rest) = path.strip_prefix("/api/file/") {
                return handle_file(app, req, rest.to_string(), query);
            }
            if let Some(rest) = path.strip_prefix("/api/folder/") {
                return handle_folder(app, req, rest.to_string(), query);
            }
            if method == Method::Get {
                return serve_static(req, &path);
            }
            not_found(req)
        }
    }
}

fn handle_file(app: Arc<Server>, req: Request, rest: String, query: Vec<(String, String)>) {
    if let Some(id) = rest.strip_suffix("/share") {
        return match app.share(id) {
            Ok(v) => json_value(req, 200, &v),
            Err(e) => fail(req, &e),
        };
    }
    if let Some(id) = rest.strip_suffix("/download") {
        return download_one(req, &app, id, &query);
    }
    if req.method() == &Method::Delete {
        return match app.delete_file(&rest) {
            Ok(v) => json_value(req, 200, &v),
            Err(e) => fail(req, &e),
        };
    }
    not_found(req)
}

fn handle_folder(app: Arc<Server>, req: Request, rest: String, query: Vec<(String, String)>) {
    if let Some(id) = rest.strip_suffix("/download") {
        return download_merged(req, &app, id, &query);
    }
    if req.method() == &Method::Delete {
        return match app.delete_folder(&rest) {
            Ok(v) => json_value(req, 200, &v),
            Err(e) => fail(req, &e),
        };
    }
    not_found(req)
}

// ---------- 下载 ----------

fn download_one(req: Request, app: &Arc<Server>, file_id: &str, query: &[(String, String)]) {
    let name = q_str(query, "name").unwrap_or_else(|| file_id.to_string());

    let url = match app.direct_link(file_id) {
        Ok(u) => u,
        Err(e) => return fail(req, &e),
    };
    let stream = match lanzou::open_direct(&url) {
        Ok(s) => s,
        Err(e) => return fail(req, &e),
    };
    // 直链被消费掉或过期时上游会回一个 HTML 页面。绝不能把它当成文件发出去 ——
    // 否则用户拿到的是名字叫 .zip、内容却是几 KB 网页的东西。
    if stream.status >= 400 {
        return fail_msg(req, 502, &format!("上游返回 HTTP {}，直链可能已失效", stream.status));
    }
    if stream.content_type.to_lowercase().starts_with("text/html") {
        return fail_msg(
            req,
            502,
            "直链已失效或被拒绝（拿到的是 HTML 页面而不是文件），请稍后重试",
        );
    }

    let length = stream.content_length.map(|n| n as usize);
    let resp = Response::new(
        StatusCode(200),
        octet_headers(&name, stream.content_length),
        stream.body,
        length,
        None,
    )
    .with_chunked_threshold(usize::MAX);
    let _ = req.respond(resp);
}

/// 把一个分卷文件夹里的所有块按序拼成一个响应发出去。
///
/// 不设 Content-Length：分卷总长必须打开每个分片的下载流才知道，而直链是一次性的
/// （先全部打开探长度，靠后的那些会在轮到它们之前就失效），所以只能走 chunked。
fn download_merged(req: Request, app: &Arc<Server>, folder_id: &str, query: &[(String, String)]) {
    let parts = match app.part_files(folder_id) {
        Ok(p) => p,
        Err(e) => return fail(req, &e),
    };
    if parts.is_empty() {
        return fail_msg(req, 400, "这个文件夹是空的，没有可合并的内容");
    }
    let name = q_str(query, "name").unwrap_or_else(|| format!("{folder_id}.bin"));

    let merged = MergedParts {
        app: Arc::clone(app),
        parts,
        cur: None,
        idx: 0,
    };
    let resp = Response::new(
        StatusCode(200),
        octet_headers(&name, None),
        merged,
        None,
        None,
    )
    .with_chunked_threshold(0);
    let _ = req.respond(resp);
}

fn octet_headers(name: &str, len: Option<u64>) -> Vec<Header> {
    let mut h = vec![
        header("Content-Type", "application/octet-stream"),
        header(
            "Content-Disposition",
            &format!("attachment; filename*=UTF-8''{}", rfc5987(name)),
        ),
    ];
    if let Some(n) = len {
        h.push(header("Content-Length", &n.to_string()));
    }
    h
}

/// 惰性的分卷拼接流：读到一块的末尾才去打开下一块。
struct MergedParts {
    app: Arc<Server>,
    parts: Vec<PartRef>,
    cur: Option<Box<dyn Read + Send>>,
    idx: usize,
}

impl Read for MergedParts {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Some(cur) = self.cur.as_mut() {
                match cur.read(buf) {
                    Ok(0) => {
                        self.cur = None;
                        self.idx += 1;
                    }
                    Ok(n) => return Ok(n),
                    Err(e) => return Err(e),
                }
            }
            if self.idx >= self.parts.len() {
                return Ok(0);
            }
            let part = &self.parts[self.idx];
            let url = self
                .app
                .direct_link(&part.id)
                .map_err(|e| io::Error::other(format!("取第 {} 块直链失败: {e}", self.idx + 1)))?;
            let stream = lanzou::open_direct(&url)
                .map_err(|e| io::Error::other(format!("第 {} 块打不开: {e}", self.idx + 1)))?;
            self.cur = Some(stream.body);
        }
    }
}

// ---------- 静态资源 ----------

fn serve_static(req: Request, path: &str) {
    // 静态托管必须放在所有 /api 路由之后，否则会抢走 /api 前缀
    let rel = match path {
        "/" => "index.html",
        p => p.trim_start_matches('/'),
    };
    match Assets::get(rel) {
        Some(f) => {
            let data = f.data.into_owned();
            let mime = mime_of(rel);
            let resp = Response::from_data(data)
                .with_status_code(200)
                .with_header(header("Content-Type", mime))
                // 默认超过 32KB 就改成 chunked，那样就没有 Content-Length 了。
                // 前端的主 JS 有两百多 KB，带长度浏览器才做得了缓存和进度。
                .with_chunked_threshold(usize::MAX);
            let _ = req.respond(resp);
        }
        None => not_found(req),
    }
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "json" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

// ---------- 响应工具 ----------

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("响应头不该含非法字符")
}

fn json(req: Request, status: u16, body: &str) {
    let resp = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_chunked_threshold(usize::MAX);
    let _ = req.respond(resp);
}

fn json_value<T: serde::Serialize>(req: Request, status: u16, v: &T) {
    let body = serde_json::to_string(v).unwrap_or_else(|_| "{}".into());
    json(req, status, &body);
}

fn fail(req: Request, err: &LzError) {
    let status = match err {
        // 两种"该弹登录卡"的情况都走 401：没配置账号、会话失效
        LzError::NotConfigured | LzError::NotLoggedIn => 401,
        // 蓝奏云或它前面那道网关自己抽风（实测遇到 504），跟请求内容无关，重试就好
        LzError::Http(_) => 502,
        _ => 400,
    };
    fail_msg(req, status, &err.to_string());
}

fn fail_msg(req: Request, status: u16, msg: &str) {
    json(req, status, &error_body(msg));
}

fn not_found(req: Request) {
    let _ = req.respond(Response::empty(404));
}

/// Host 头是不是指向本机。
///
/// 要处理三种写法：`127.0.0.1:8790`、`localhost:8790`、`[::1]:8790`。
/// 之前把 IPv6 的方括号剥错了（`[::1]` 会被切成空串），单独拎出来测。
fn is_loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        // [::1]:8790 → ::1
        Some(rest) => rest.split(']').next().unwrap_or(""),
        // 127.0.0.1:8790 → 127.0.0.1；localhost → localhost
        None => host.split(':').next().unwrap_or(""),
    };
    matches!(name, "127.0.0.1" | "localhost" | "::1")
}

/// 读一个小 JSON 请求体。
fn read_json(req: &mut Request, limit: u64) -> Result<serde_json::Value, String> {
    let body = req
        .as_reader()
        .bytes()
        .take(limit as usize)
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|e| format!("读取请求体失败: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("请求格式不对: {e}"))
}

// ---------- URL / query ----------

pub fn split_url(raw: &str) -> (String, Vec<(String, String)>) {
    match raw.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_query(q)),
        None => (raw.to_string(), Vec::new()),
    }
}

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (dec(k), dec(v)),
            None => (dec(pair), String::new()),
        })
        .collect()
}

fn dec(s: &str) -> String {
    percent_encoding::percent_decode_str(&s.replace('+', " "))
        .decode_utf8_lossy()
        .to_string()
}

fn q_str(q: &[(String, String)], key: &str) -> Option<String> {
    q.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn q_i64(q: &[(String, String)], key: &str, def: i64) -> i64 {
    q_str(q, key).and_then(|v| v.parse().ok()).unwrap_or(def)
}

/// JSON 字符串转义（只用来拼一两个小响应，不值得引别的）。
pub fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parsing_decodes_and_handles_plus() {
        let q = parse_query("filename=%E7%94%B5%E5%BD%B1.zip&folder_id=-1&name=a+b");
        assert_eq!(q_str(&q, "filename").unwrap(), "电影.zip");
        assert_eq!(q_i64(&q, "folder_id", 0), -1);
        assert_eq!(q_str(&q, "name").unwrap(), "a b");
    }

    #[test]
    fn missing_query_falls_back_to_default() {
        let q = parse_query("");
        assert_eq!(q_i64(&q, "pg", 0), 0);
        assert!(q_str(&q, "name").is_none());
    }

    #[test]
    fn host_validation_accepts_only_loopback() {
        for ok in ["127.0.0.1:8790", "127.0.0.1", "localhost:8790", "localhost", "[::1]:8790", "[::1]"] {
            assert!(is_loopback_host(ok), "{ok} 应该放行");
        }
        for bad in [
            "evil.com:8790",
            "evil.com",
            "attacker.example",
            // 这几个是想钻空子的写法，必须挡住
            "127.0.0.1.evil.com",
            "localhost.evil.com",
            "notlocalhost",
            "",
        ] {
            assert!(!is_loopback_host(bad), "{bad} 应该拒绝");
        }
    }

    #[test]
    fn url_split() {
        let (p, q) = split_url("/api/list?folder_id=-1");
        assert_eq!(p, "/api/list");
        assert_eq!(q_i64(&q, "folder_id", 0), -1);

        let (p2, q2) = split_url("/api/ping");
        assert_eq!(p2, "/api/ping");
        assert!(q2.is_empty());
    }

    #[test]
    fn rfc5987_encodes_chinese_and_spaces() {
        let e = rfc5987("电影 测试.zip");
        assert!(!e.contains(' '), "空格必须编码: {e}");
        assert!(e.starts_with("%E7%94%B5"), "中文要按 UTF-8 逐字节编码: {e}");
        assert!(e.ends_with(".zip"));
    }
}
