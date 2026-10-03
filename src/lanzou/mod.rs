//! 蓝奏云 SDK：会话、登录、各 task 接口、直链、分卷。

pub mod client;
pub mod convert;
pub mod cookie;
pub mod session;

pub use client::{open_direct, Client, ROOT};
pub use convert::str_of;

/// SDK 层面的错误。`NotLoggedIn` 单独拎出来 —— 调用方要靠它触发重登。
#[derive(Debug)]
pub enum LzError {
    /// 还没配置账号密码 —— 界面该弹登录卡，MCP 该提示去打开一次应用
    NotConfigured,
    /// 服务端返回 zt=9，会话失效
    NotLoggedIn,
    /// 业务失败，`info` 里是中文原因
    Api(String),
    /// 网络/传输层
    Http(String),
    /// 其它（解析失败、结构变化等）
    Other(String),
}

impl std::fmt::Display for LzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LzError::NotConfigured => {
                write!(f, "还没配置网盘账号，请先打开一次应用登录（或设 LANZOU_USER / LANZOU_PWD）")
            }
            LzError::NotLoggedIn => write!(f, "登录状态已失效"),
            LzError::Api(s) | LzError::Http(s) | LzError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for LzError {}
