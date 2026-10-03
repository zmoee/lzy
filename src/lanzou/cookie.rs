//! 会话 cookie 的存盘与恢复。
//!
//! 为什么需要它：不存的话，**每一次 lzy 调用都要重新登录**（登录页 → 过挑战 →
//! POST 账号密码 → 跟一次性令牌，四个请求），而蓝奏云会把短时间内的大量登录
//! 判成异常客户端，直接回 403/409。实测连跑十几个命令就会被限流。
//!
//! 而"被 AI 连续调用"正是这个工具的主要用法，所以这里不是优化，是必需品。
//! 存盘之后每次调用只剩一个业务请求。
//!
//! 注意：cookie 等同于账号凭据（phpdisk_info 就是鉴权唯一凭据），
//! 这个文件别外传。

use std::path::PathBuf;

use crate::lanzou::session::Session;
use reqwest::cookie::CookieStore;

use crate::lanzou::client::UP_BASE;

const ACC_BASE: &str = "https://accounts.woozooo.com";

/// 用户数据目录的基路径。
///
/// - Windows: `%APPDATA%`
/// - Linux:   `$XDG_DATA_HOME`，没设就用 `~/.local/share`
///   （**不能直接用 `$HOME`** —— 那样会落在 `~/lzy`，而项目目录很可能正好也叫
///   `lzy`，凭据文件就躺进源码树里了。这个坑实际踩到过。）
/// - 兜底：临时目录。**绝不用 `"."`** —— 双击启动时当前目录就是 exe 所在目录。
pub fn data_base() -> PathBuf {
    if let Ok(p) = std::env::var("APPDATA") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(p) = std::env::var("XDG_DATA_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h).join(".local").join("share");
        }
    }
    std::env::temp_dir()
}

/// cookie 文件位置。可以用 LANZOU_COOKIE_FILE 覆盖（测试用）。
pub fn cookie_path() -> PathBuf {
    if let Ok(p) = std::env::var("LANZOU_COOKIE_FILE") {
        return PathBuf::from(p);
    }
    data_base().join("lzy").join("cookies.json")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    /// 这份 cookie 属于哪个账号。不带上的话，换了 LANZOU_USER 之后
    /// 会错误地恢复上一个账号的会话，然后在"别人的"网盘里操作。
    user: String,
    up: String,
    acc: String,
    /// 上次存盘时间（Unix 秒），只用来给人看
    saved_at: u64,
}

fn header_for(sess: &Session, url: &str) -> String {
    use reqwest::Url;
    if let Ok(u) = Url::parse(url) {
        if let Some(v) = sess.jar.cookies(&u) {
            if let Ok(s) = v.to_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn apply(sess: &Session, url: &str, header: &str) {
    use reqwest::Url;
    let Ok(u) = Url::parse(url) else { return };
    let Some(host) = u.host_str() else { return };
    for pair in header.split(';') {
        let pair = pair.trim();
        if pair.is_empty() || !pair.contains('=') {
            continue;
        }
        sess.jar
            .add_cookie_str(&format!("{pair}; Domain={host}; Path=/"), &u);
    }
}

/// 把当前会话的 cookie 写到磁盘。写失败不算致命（下次重新登录就是），所以只警告。
pub fn save(sess: &Session, user: &str) {
    let stored = Stored {
        user: user.to_string(),
        up: header_for(sess, UP_BASE),
        acc: header_for(sess, ACC_BASE),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    if stored.up.is_empty() {
        return;
    }
    let path = cookie_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string(&stored) {
        Ok(body) => {
            if let Err(e) = std::fs::write(&path, body) {
                note!("[lzy] 会话 cookie 写盘失败（不影响使用，下次会重新登录）: {e}");
            }
        }
        Err(e) => note!("[lzy] 会话 cookie 序列化失败: {e}"),
    }
}

/// 从磁盘恢复会话，返回里面的 uid（恢复不成功就返回空串，调用方会去登录）。
///
/// 只恢复属于 `user` 的那一份 —— 换账号后必须重新登录，不能拿旧账号的会话去用。
pub fn restore(sess: &Session, user: &str) -> String {
    let path = cookie_path();
    let Ok(body) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    let Ok(stored) = serde_json::from_str::<Stored>(&body) else {
        return String::new();
    };
    if stored.user != user {
        return String::new();
    }

    apply(sess, UP_BASE, &stored.up);
    apply(sess, ACC_BASE, &stored.acc);

    // 用 uid 判断这套 cookie 还有没有用
    use reqwest::Url;
    let Ok(u) = Url::parse(UP_BASE) else {
        return String::new();
    };
    sess.read_cookie(&u, "ylogin").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_cookie_header() {
        let sess = Session::new().expect("建会话");
        let url = UP_BASE;
        apply(&sess, url, "ylogin=1234567; phpdisk_info=abc%3D; acw_sc__v2=DEAD");

        use reqwest::Url;
        let u = Url::parse(url).unwrap();
        assert_eq!(sess.read_cookie(&u, "ylogin").as_deref(), Some("1234567"));
        assert_eq!(sess.read_cookie(&u, "acw_sc__v2").as_deref(), Some("DEAD"));
        // 再取出来应该和存进去的一致
        assert!(header_for(&sess, url).contains("ylogin=1234567"));
    }

    #[test]
    fn other_account_cookie_is_not_reused() {
        let path = "/tmp/lzy-cookie-acct-test.json";
        std::env::set_var("LANZOU_COOKIE_FILE", path);
        std::fs::write(
            path,
            r#"{"user":"alice","up":"ylogin=1","acc":"","saved_at":0}"#,
        )
        .unwrap();

        let sess = Session::new().unwrap();
        assert_eq!(restore(&sess, "bob"), "", "换了账号就不该复用旧会话");
        assert_eq!(restore(&sess, "alice"), "1", "同一个账号才复用");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        std::env::set_var("LANZOU_COOKIE_FILE", "/tmp/lzy-nonexistent-cookie.json");
        let sess = Session::new().expect("建会话");
        assert_eq!(restore(&sess, "someone"), "");
    }
}
