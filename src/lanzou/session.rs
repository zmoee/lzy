//! 会话层：cookie 隔离、ESA 挑战重放、请求超时。
//!
//! 蓝奏云的三套域名（账号站 `.woozooo.com`、分享页 `*.lanzn.com`、
//! 下载域 `*.lanrar.com`）cookie 互不通用 —— reqwest 的 cookie jar
//! 天然按 host 隔离，正好对上。

use std::sync::Arc;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::{HeaderMap, ACCEPT_LANGUAGE, REFERER};
use reqwest::Url;

use crate::acw;
use crate::lanzou::LzError;

pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 响应体读取上限。挑战页约 4.3 KB，列表页百来 KB，
/// 上传响应里带一个文件条目 —— 16 MB 足够宽松又能防住异常响应吃内存。
const MAX_BODY: u64 = 16 << 20;

pub struct Session {
    pub client: Client,
    pub jar: Arc<Jar>,
}

/// 已经读完的响应。非流式请求统一走这个形状。
pub struct Resp {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// 组装请求时额外要带的头。逐个对齐实测出来的组合，
/// 少一个都可能被风控当成异常客户端。
#[derive(Default, Clone)]
pub struct ReqOpts {
    pub referer: Option<String>,
    pub origin: Option<String>,
    pub ajax: bool,
    pub timeout: Option<Duration>,
}

impl Session {
    pub fn new() -> Result<Self, LzError> {
        let jar = Arc::new(Jar::default());
        let client = Client::builder()
            .cookie_provider(jar.clone())
            .user_agent(UA)
            .build()
            .map_err(|e| LzError::Http(e.to_string()))?;
        Ok(Self { client, jar })
    }

    /// 发一个请求并把响应体读完。
    ///
    /// 超时设在**请求上**而不是 client 上：reqwest 的请求超时覆盖到
    /// "响应体读完"为止，所以这里不需要像 Go 版那样操心 context 的
    /// cancel 作用域（那边踩过：Do 一返回就 cancel，紧接着读 body 会报错）。
    pub fn exec(&self, req: reqwest::blocking::RequestBuilder) -> Result<Resp, LzError> {
        let resp = req.send().map_err(|e| LzError::Http(e.to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = read_limited(resp, MAX_BODY)?;
        Ok(Resp { status, headers, body })
    }

    pub fn get(&self, url: &str, timeout: Duration) -> Result<Resp, LzError> {
        let req = self
            .client
            .get(url)
            .timeout(timeout)
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9");
        self.exec(req)
    }

    pub fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
        opts: ReqOpts,
    ) -> Result<Resp, LzError> {
        let mut req = self
            .client
            .post(url)
            .form(form)
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9");
        req = apply_opts(req, &opts);
        if let Some(t) = opts.timeout {
            req = req.timeout(t);
        }
        self.exec(req)
    }

    /// GET 但**不读 body**，交给调用方流式消费（下载用）。
    pub fn get_stream(&self, url: &str) -> Result<Response, LzError> {
        self.client
            .get(url)
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
            .send()
            .map_err(|e| LzError::Http(e.to_string()))
    }

    /// 抓一个页面，自动过挑战。
    ///
    /// 最多重放一次：第一次拿到挑战页就解出来写 cookie 再请求一次；
    /// 第二次还拿到挑战页说明算法失效了，直接报错而不是把混淆脚本当正文返回。
    pub fn get_text(&self, url: &str, timeout: Duration) -> Result<String, LzError> {
        for attempt in 0..2 {
            let res = self.get(url, timeout)?;
            let text = String::from_utf8_lossy(&res.body).to_string();

            match acw::solve_html(&text) {
                Err(acw::AcwError::NotAChallenge) => return Ok(text),
                Err(e) => return Err(LzError::Other(format!("挑战页解析失败: {e}"))),
                Ok(sig) => {
                    self.set_challenge_cookie(url, &sig)?;
                    if attempt == 1 {
                        return Err(LzError::Other(
                            "反复命中阿里云 ESA 挑战，acw 算法可能已失效".into(),
                        ));
                    }
                }
            }
        }
        unreachable!()
    }

    /// 把解出来的 acw_sc__v2 绑到本次请求的 host 上。
    ///
    /// 必须逐 host 绑定：账号站、分享页、下载域各有一套挑战，写错域等于没解。
    /// 写完读回来确认 —— cookie jar 对不合规的域是静默丢弃的，
    /// 否则表现就是"每个请求都命中挑战"，很难查。
    pub fn set_challenge_cookie(&self, url_str: &str, value: &str) -> Result<(), LzError> {
        let url = Url::parse(url_str).map_err(|e| LzError::Other(format!("URL 解析失败: {e}")))?;
        let host = url
            .host_str()
            .ok_or_else(|| LzError::Other("URL 没有 host".into()))?
            .to_string();

        let cookie = format!("acw_sc__v2={value}; Domain={host}; Path=/");
        self.jar.add_cookie_str(&cookie, &url);

        if self.read_cookie(&url, "acw_sc__v2").as_deref() == Some(value) {
            Ok(())
        } else {
            Err(LzError::Other(
                "acw_sc__v2 cookie 没写进 cookie jar（域不匹配）".into(),
            ))
        }
    }

    /// 从 jar 里读某个 cookie 的值。
    pub fn read_cookie(&self, url: &Url, name: &str) -> Option<String> {
        let raw = self.jar.cookies(url)?;
        let s = raw.to_str().ok()?;
        for pair in s.split(';') {
            let pair = pair.trim();
            if let Some(v) = pair.strip_prefix(&format!("{name}=")) {
                return Some(v.to_string());
            }
        }
        None
    }
}

fn apply_opts(
    mut req: reqwest::blocking::RequestBuilder,
    opts: &ReqOpts,
) -> reqwest::blocking::RequestBuilder {
    if opts.ajax {
        req = req.header("X-Requested-With", "XMLHttpRequest");
    }
    if let Some(r) = &opts.referer {
        req = req.header(REFERER, r);
    }
    if let Some(o) = &opts.origin {
        req = req.header("Origin", o);
    }
    req
}

fn read_limited(resp: Response, limit: u64) -> Result<Vec<u8>, LzError> {
    use std::io::Read;
    let mut buf = Vec::new();
    resp.take(limit)
        .read_to_end(&mut buf)
        .map_err(|e| LzError::Http(e.to_string()))?;
    Ok(buf)
}

pub fn header_str(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}
