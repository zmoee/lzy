//! 蓝奏云（woozooo）账号站与后台接口封装。
//!
//! 鉴权模型：账号站登录后服务端下发 `phpdisk_info` / `ylogins` 等 cookie，
//! 后续 `doupload.php` / `html5up.php` 只认 cookie。
//!
//! 会话用一把锁串行化：蓝奏云的会话语义（一次性跳转令牌、换直链时的中间态）
//! 经不起并发交错。

use std::io::{Cursor, Read};
use std::sync::Mutex;
use std::time::Duration;

use regex::Regex;
use reqwest::blocking::multipart::{Form, Part};
use reqwest::Url;
use serde_json::Value;

use super::convert::*;
use super::session::{ReqOpts, Resp, Session};
use super::LzError;

pub const UP_BASE: &str = "https://up.woozooo.com";
pub const ACC_BASE: &str = "https://accounts.woozooo.com";
const AJAX_FILE: &str = "https://apifile.woozooo.com/ajaxfile.php";

/// 根目录的 folder_id。
pub const ROOT: i64 = -1;

pub const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
pub const API_TIMEOUT: Duration = Duration::from_secs(60);
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

fn index_url() -> String {
    format!("{UP_BASE}/mydisk.php?item=files&action=index")
}

pub struct Client {
    user: String,
    pwd: String,
    inner: Mutex<Inner>,
}

struct Inner {
    sess: Session,
    uid: String,
}

impl Client {
    pub fn new(user: &str, pwd: &str) -> Result<Self, LzError> {
        let sess = Session::new()?;
        // 先用上次存下来的会话试试。命中就不用登录了 —— 这是必须的：
        // 每次调用都重新登录的话，连跑十几个命令就会被服务端限流（实测 403/409）。
        let uid = super::cookie::restore(&sess, user);
        Ok(Self {
            user: user.to_string(),
            pwd: pwd.to_string(),
            inner: Mutex::new(Inner { sess, uid }),
        })
    }

    /// 全部调用的唯一入口：持锁 → 确保已登录 → 执行 → 会话失效则重登一次再来。
    fn with_session<T, F>(&self, f: F) -> Result<T, LzError>
    where
        F: Fn(&Session) -> Result<T, LzError>,
    {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        if inner.uid.is_empty() {
            self.login_locked(&mut inner)?;
        }

        let first = f(&inner.sess);
        if matches!(first, Err(LzError::NotLoggedIn)) {
            inner.uid.clear();
            self.login_locked(&mut inner)?;
            return f(&inner.sess);
        }
        first
    }

    pub fn uid(&self) -> Result<String, LzError> {
        self.with_session(|_| Ok(String::new()))?; // 保证已登录
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Ok(inner.uid.clone())
    }

    pub fn relogin(&self) -> Result<String, LzError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.uid.clear();
        self.login_locked(&mut inner)?;
        Ok(inner.uid.clone())
    }

    fn login_locked(&self, inner: &mut Inner) -> Result<(), LzError> {
        let login_page = format!("{ACC_BASE}/accounts.php?action=login&ref=up.woozooo.com");

        // 先过账号站的 ESA 挑战，否则登录页只会返回 4KB 混淆脚本
        inner.sess.get_text(&login_page, PAGE_TIMEOUT)?;

        // 密码明文提交，无 hash、无签名、无验证码 —— 这是蓝奏云自己的设计
        let res = inner.sess.post_form(
            &format!("{ACC_BASE}/accounts.php"),
            &[
                ("task", "uselogin"),
                ("username", &self.user),
                ("password", &self.pwd),
                ("ref", "up.woozooo.com"),
            ],
            ReqOpts {
                referer: Some(login_page),
                origin: Some(ACC_BASE.to_string()),
                ajax: true,
                timeout: Some(PAGE_TIMEOUT),
            },
        )?;

        let obj = decode_json(&res, "登录接口")?;
        if zt_of(&obj) != "1" {
            let msg = first_non_empty(&[str_of(&obj, "msgs"), "账号或密码错误".into()]);
            return Err(LzError::Api(format!("登录失败: {msg}")));
        }

        // 返回的跳转令牌是一次性的：必须跟随一次，才能在 up 域建立会话
        let jump = str_of(&obj, "msgs");
        if jump.is_empty() {
            return Err(LzError::Other("登录响应里没有跳转地址".into()));
        }
        inner.sess.get_text(&jump, PAGE_TIMEOUT)?;

        let up = Url::parse(UP_BASE).expect("常量 URL");
        let uid = inner.sess.read_cookie(&up, "ylogin").unwrap_or_default();
        if uid.is_empty() {
            return Err(LzError::Other("登录后没拿到 ylogin cookie，会话无效".into()));
        }
        inner.uid = uid;
        // 登录成功就把 cookie 存下来，下次调用直接复用，不再重复登录
        super::cookie::save(&inner.sess, &self.user);
        Ok(())
    }

    // ---------- 底层调用 ----------

    fn task(&self, id: i64, params: &[(&str, String)]) -> Result<Value, LzError> {
        let owned: Vec<(String, String)> =
            params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        self.with_session(|sess| self.task_on(sess, id, &owned))
    }

    /// 调 doupload.php 的某一个 task。
    ///
    /// **调用方必须已经持有会话**（即从 `with_session` 的闭包里调进来）。
    /// 千万别在这里再调 `with_session` —— std::sync::Mutex 不可重入，
    /// 那样会自己把自己锁死，而且现象是"命令卡住不动"，很难查。
    fn task_on(
        &self,
        sess: &Session,
        id: i64,
        params: &[(String, String)],
    ) -> Result<Value, LzError> {
        let task_id = id.to_string();
        let mut form: Vec<(&str, &str)> = vec![("task", task_id.as_str())];
        for (k, v) in params {
            form.push((k.as_str(), v.as_str()));
        }

        let res = sess.post_form(
            &format!("{UP_BASE}/doupload.php"),
            &form,
            ReqOpts {
                referer: Some(index_url()),
                ajax: true,
                timeout: Some(API_TIMEOUT),
                ..Default::default()
            },
        )?;
        let obj = decode_json(&res, "doupload 接口")?;
        if zt_of(&obj) == "9" {
            return Err(LzError::NotLoggedIn);
        }
        Ok(obj)
    }

    // ---------- 读接口 ----------

    /// 文件列表（task=5）。pg 从 1 开始。
    pub fn files(&self, folder_id: i64, pg: i64) -> Result<Vec<Value>, LzError> {
        let obj = self.task(
            5,
            &[
                ("folder_id", folder_id.to_string()),
                ("pg", pg.to_string()),
            ],
        )?;
        Ok(list_of(&obj, "text"))
    }

    /// task=47：一次拿到某目录的子文件夹与当前路径（面包屑）。
    ///
    /// 两者字段名不同：text[] 用 `fol_id`，info[] 用 `folderid`。
    /// 另外根目录返回 zt=1、进入子目录返回 zt=2，都不是错误，不能校验 zt。
    pub fn browse(&self, folder_id: i64) -> Result<(Vec<Value>, Vec<Value>), LzError> {
        let obj = self.task(47, &[("folder_id", folder_id.to_string())])?;
        Ok((list_of(&obj, "text"), list_of(&obj, "info")))
    }

    /// 文件分享信息（task=22）：`f_id` 分享短码、`pwd`、`is_newd` 分享域名。
    pub fn file_info(&self, file_id: &str) -> Result<Value, LzError> {
        let id = file_id.to_string();
        self.with_session(|sess| self.file_info_on(sess, &id))
    }

    /// 同 task_on：调用方必须已持有会话。
    fn file_info_on(&self, sess: &Session, file_id: &str) -> Result<Value, LzError> {
        let obj = self.task_on(
            sess,
            22,
            &[("file_id".to_string(), file_id.to_string())],
        )?;
        ok_or_api(&obj)?;
        let info = obj
            .get("info")
            .cloned()
            .ok_or_else(|| LzError::Other("接口响应里没有 info 对象".into()))?;

        // 坑：对一个不存在的 file_id，task=22 会返回 zt=1 加一串看着像模像样的
        // 垃圾数据（实测给的是 f_id="i"、pwd="d8zb"），顺着用就会生成一个
        // 点开 404 的假分享链接。真实分享短码是 12 位，这里用宽松下限兜住。
        let short = str_of(&info, "f_id");
        if !short.is_empty() && short.len() < 8 {
            return Err(LzError::Api(
                "文件不存在或已被删除（蓝奏云对无效 id 返回了假数据）".into(),
            ));
        }
        Ok(info)
    }

    /// 自定义分享域名列表（task=49 type=1）。
    pub fn domain_list(&self) -> Result<Vec<Value>, LzError> {
        let obj = self.task(49, &[("type", "1".to_string())])?;
        ok_or_api(&obj)?;
        Ok(list_of(&obj, "text"))
    }

    // ---------- 写接口 ----------

    pub fn del_file(&self, file_id: &str) -> Result<Value, LzError> {
        let obj = self.task(6, &[("file_id", file_id.to_string())])?;
        ok_or_api(&obj)
    }

    pub fn del_folder(&self, folder_id: &str) -> Result<Value, LzError> {
        let obj = self.task(3, &[("folder_id", folder_id.to_string())])?;
        ok_or_api(&obj)
    }

    /// 新建文件夹（task=2），返回新 fol_id。
    ///
    /// 注意 parent_id 的口径和列表接口不一样：列表里根目录是 -1，
    /// 但新建文件夹时根目录要传 0。
    pub fn new_folder(&self, name: &str, parent_id: i64, desc: &str) -> Result<String, LzError> {
        let obj = self.task(
            2,
            &[
                ("parent_id", parent_id.to_string()),
                ("folder_name", name.to_string()),
                ("folder_description", desc.to_string()),
            ],
        )?;
        let obj = ok_or_api(&obj)?;
        let id = str_of(&obj, "text");
        if id.is_empty() {
            return Err(LzError::Other("新建文件夹成功但没返回 fol_id".into()));
        }
        Ok(id)
    }


    /// 移动文件到目标文件夹（task=20）。
    ///
    /// 上传接口（html5up.php）不认任何形式的 folder_id —— 实测把 folder_id 放
    /// 表单字段、URL 查询串，还是换成 folder_id_bibao / fol_id / parent_id，
    /// 全部落根目录。所以"上传到指定文件夹"只能靠上传完再移动。
    pub fn move_file(&self, file_id: &str, folder_id: i64) -> Result<Value, LzError> {
        let obj = self.task(
            20,
            &[
                ("file_id", file_id.to_string()),
                ("folder_id", folder_id.to_string()),
            ],
        )?;
        ok_or_api(&obj)
    }

    /// 上传一段内存里的内容（html5up.php multipart）。
    pub fn upload_bytes(
        &self,
        filename: &str,
        data: Vec<u8>,
        folder_id: i64,
    ) -> Result<Value, LzError> {
        let name = filename.to_string();
        self.with_session(move |sess| {
            let form = Form::new()
                .text("task", "1")
                .text("vie", "2")
                .text("ve", "2")
                .text("folder_id", folder_id.to_string())
                .part(
                    "upload_file",
                    Part::bytes(data.clone()).file_name(name.clone()),
                );

            let res = sess
                .client
                .post(format!("{UP_BASE}/html5up.php"))
                .multipart(form)
                .header("Referer", index_url())
                .header("Origin", UP_BASE)
                .timeout(UPLOAD_TIMEOUT)
                .send()
                .map_err(|e| LzError::Http(e.to_string()))?;

            let status = res.status().as_u16();
            let headers = res.headers().clone();
            let mut body = Vec::new();
            res.take(MAX_UPLOAD_RESP)
                .read_to_end(&mut body)
                .map_err(|e| LzError::Http(e.to_string()))?;
            let r = Resp { status, headers, body };

            let obj = decode_json(&r, "上传接口")?;
            if zt_of(&obj) == "9" {
                return Err(LzError::NotLoggedIn);
            }
            if zt_of(&obj) != "1" {
                let msg = first_non_empty(&[str_of(&obj, "info"), format!("zt={}", zt_of(&obj))]);
                return Err(LzError::Api(format!("上传失败: {msg}")));
            }
            let items = list_of(&obj, "text");
            items
                .into_iter()
                .next()
                .ok_or_else(|| LzError::Other("上传成功但响应里没有文件条目".into()))
        })
    }

    // ---------- 直链 ----------

    /// 换出真实下载直链。
    ///
    /// 链路：task=22 拿分享短码 → 打开分享页取 sign → POST apifile.woozooo.com
    /// → `{dom, url}` → 直链 = `{dom}/file/{url}`。
    pub fn direct_link(&self, file_id: &str) -> Result<String, LzError> {
        let id = file_id.to_string();
        self.with_session(|sess| self.direct_link_on(sess, &id))
    }

    /// 同 task_on：调用方必须已持有会话。
    fn direct_link_on(&self, sess: &Session, file_id: &str) -> Result<String, LzError> {
        let info = self.file_info_on(sess, file_id)?;
        let domain = str_of(&info, "is_newd");
        let domain = domain.trim_end_matches('/').to_string();
        let short = str_of(&info, "f_id");
        if domain.is_empty() || short.is_empty() {
            return Err(LzError::Api(format!("无法取得分享地址: {info}")));
        }
        let pwd = if str_of(&info, "onof") == "1" {
            str_of(&info, "pwd")
        } else {
            String::new()
        };
        let share = format!("{domain}/{short}");

        // 下载链走独立会话：分享页与账号站的 cookie 互不通用，分开更干净
        let dl = Session::new()?;
        let page = dl.get_text(&share, PAGE_TIMEOUT)?;

        let re_isngis = Regex::new(r"var isngis = '([^']+)'").unwrap();
        let re_fn = Regex::new(r#"src="(/fn\?[^"]+)""#).unwrap();
        let re_wp = Regex::new(r"var\s+wp_sign\s*=\s*'([^']+)'").unwrap();

        let sign = match re_isngis.captures(&page) {
            Some(c) => c[1].to_string(),
            None => {
                let fn_path = re_fn
                    .captures(&page)
                    .map(|c| c[1].to_string())
                    .ok_or_else(|| LzError::Api("分享页结构变化，未找到下载入口".into()))?;
                let fn_page = dl.get_text(&format!("{domain}{fn_path}"), PAGE_TIMEOUT)?;
                re_wp
                    .captures(&fn_page)
                    .map(|c| c[1].to_string())
                    .ok_or_else(|| LzError::Api("未取得 sign".into()))?
            }
        };

        let res = dl.post_form(
            &format!("{AJAX_FILE}?file={file_id}"),
            &[
                ("action", "downprocess"),
                ("sign", &sign),
                ("kd", "1"),
                ("p", &pwd),
            ],
            ReqOpts {
                referer: Some(share),
                timeout: Some(API_TIMEOUT),
                ..Default::default()
            },
        )?;

        let obj = decode_json(&res, "apifile 接口")?;
        if zt_of(&obj) != "1" {
            let msg = first_non_empty(&[
                str_of(&obj, "inf"),
                str_of(&obj, "info"),
                "获取直链失败".into(),
            ]);
            return Err(LzError::Api(msg));
        }
        let dom = str_of(&obj, "dom");
        let path = str_of(&obj, "url");
        if dom.is_empty() || path.is_empty() {
            return Err(LzError::Other("直链响应缺少 dom/url".into()));
        }
        Ok(format!("{dom}/file/{path}"))
    }
}

const MAX_UPLOAD_RESP: u64 = 16 << 20;

/// 要求 zt=1，否则把 info 里的中文错误抛出去。
fn ok_or_api(obj: &Value) -> Result<Value, LzError> {
    let zt = zt_of(obj);
    if zt != "1" {
        let msg = first_non_empty(&[str_of(obj, "info"), format!("接口返回 zt={zt}")]);
        return Err(LzError::Api(msg));
    }
    Ok(obj.clone())
}

fn decode_json(res: &Resp, what: &str) -> Result<Value, LzError> {
    serde_json::from_slice(&res.body).map_err(|_| {
        LzError::Api(format!(
            "{what}返回不是 JSON (HTTP {}, Content-Type={:?}): {}",
            res.status,
            super::session::header_str(&res.headers, "content-type"),
            String::from_utf8_lossy(&res.body[..res.body.len().min(200)])
        ))
    })
}

// ---------- 直链的流式打开 ----------

/// 一条已经打开、可以直接往出拷贝的直链流。
pub struct DirectStream {
    pub status: u16,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

/// 打开已经换出来的直链。
///
/// 这条链接只经得起一次真实数据请求，所以不能"先探一下再重连"：
/// 先读最多 8 KB 判断开头是不是挑战页 —— 是就重放一次；不是就把这 8 KB
/// 用 `Chain` 接回原流，字节一个不少，`Content-Length` 也不用扣减。
pub fn open_direct(url: &str) -> Result<DirectStream, LzError> {
    let sess = Session::new()?;
    let resp = sess.get_stream(url)?;

    let ct = super::session::header_str(resp.headers(), "content-type");
    let cl = resp.content_length();
    let status = resp.status().as_u16();

    let mut resp = resp;
    let mut head = vec![0u8; 8192];
    let mut filled = 0usize;
    while filled < head.len() {
        match resp.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) => return Err(LzError::Http(e.to_string())),
        }
    }
    head.truncate(filled);

    if !contains(&head, b"arg1=") {
        // 正常文件流：把预读的这段接回去
        let body: Box<dyn Read + Send> = Box::new(Cursor::new(head).chain(resp));
        return Ok(DirectStream { status, content_type: ct, content_length: cl, body });
    }

    // 是挑战页：必须读完才拿得到 arg1
    let mut page = head;
    resp.take(MAX_PAGE_BYTES)
        .read_to_end(&mut page)
        .map_err(|e| LzError::Http(e.to_string()))?;
    let text = String::from_utf8_lossy(&page).to_string();
    let sig = crate::acw::solve_html(&text)
        .map_err(|e| LzError::Other(format!("下载页挑战解析失败: {e}")))?;
    sess.set_challenge_cookie(url, &sig)?;

    let resp2 = sess.get_stream(url)?;
    let ct2 = super::session::header_str(resp2.headers(), "content-type");
    let cl2 = resp2.content_length();
    let status2 = resp2.status().as_u16();
    Ok(DirectStream {
        status: status2,
        content_type: ct2,
        content_length: cl2,
        body: Box::new(resp2),
    })
}

const MAX_PAGE_BYTES: u64 = 16 << 20;

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
