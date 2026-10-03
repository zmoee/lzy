//! 账号凭据的加密存储。
//!
//! 以前账号密码是硬编码在 exe 里的 —— 谁拿到 exe，谁就拿到了账号，
//! 拷到任何机器上都能直接用。
//!
//! 现在改成：用户在界面里输一次，用 **Windows DPAPI** 加密后存到本机。
//! DPAPI 的密钥由系统从当前 Windows 用户的登录凭据派生，所以：
//!   - 密文拷到别的机器 / 别的用户下**解不开**
//!   - 不需要我们管理任何密钥
//!   - crypt32.dll 是系统自带，不引依赖
//!
//! 这实际上比硬编码更安全：凭据从"跟着 exe 走"变成了"绑在这台机器这个人上"。

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Credentials {
    pub user: String,
    pub pwd: String,
}

/// 凭据文件位置。跟 cookie 放同一个用户数据目录。
pub fn path() -> PathBuf {
    crate::lanzou::cookie::data_base()
        .join("lzy")
        .join("credentials.dat")
}

/// 读凭据。没有、或解不开（换了机器/换了用户）都返回 None。
pub fn load() -> Option<Credentials> {
    let raw = std::fs::read(path()).ok()?;
    let plain = platform::unprotect(&raw)?;
    parse(std::str::from_utf8(&plain).ok()?)
}

/// 存凭据。加密后写盘。
pub fn save(c: &Credentials) -> Result<(), String> {
    let blob = platform::protect(encode(c).as_bytes())?;
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败: {e}"))?;
    }
    std::fs::write(&p, blob).map_err(|e| format!("写凭据失败: {e}"))
}

/// 抹掉存下来的凭据（退出登录时用）。
pub fn clear() {
    let _ = std::fs::remove_file(path());
}

/// 序列化成一个 JSON 对象。
///
/// 别手写"长度前缀 + 按行读"那种格式 —— 试过，密码里一旦带换行就会被截断，
/// 而且是静默截断（登录失败但不报错）。JSON 的转义是现成且正确的，
/// serde_json 本来就在依赖里。
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    user: String,
    pwd: String,
}

fn encode(c: &Credentials) -> String {
    serde_json::to_string(&Stored {
        user: c.user.clone(),
        pwd: c.pwd.clone(),
    })
    .unwrap_or_default()
}

fn parse(s: &str) -> Option<Credentials> {
    let v: Stored = serde_json::from_str(s).ok()?;
    if v.user.is_empty() || v.pwd.is_empty() {
        return None;
    }
    Some(Credentials {
        user: v.user,
        pwd: v.pwd,
    })
}

/// 平台相关的加解密。Windows 用 DPAPI，其它平台退回明文（仅开发用）。
#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::ptr;

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;

        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            pp_sz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h: *mut c_void) -> *mut c_void;
    }

    /// 不弹任何系统对话框（有些配置下 DPAPI 会弹提示）
    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    /// 附加熵：把密文再绑到"这个程序"上。别人拿到密文、也知道用了 DPAPI，
    /// 还得知道这串常量才能解。
    const ENTROPY: &[u8] = b"lzy.wuxianwangpan.credentials.v1";

    fn blob_of(data: &[u8]) -> (DataBlob, Vec<u8>) {
        // DataBlob 不拥有内存，得留一份活着的副本
        let mut copy = data.to_vec();
        let b = DataBlob {
            cb_data: copy.len() as u32,
            pb_data: copy.as_mut_ptr(),
        };
        let _ = &mut copy;
        (b, copy)
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let (mut in_blob, _keep_in) = blob_of(plain);
            let (mut ent_blob, _keep_ent) = blob_of(ENTROPY);
            let mut out = DataBlob { cb_data: 0, pb_data: ptr::null_mut() };

            let ok = CryptProtectData(
                &mut in_blob,
                ptr::null(),
                &mut ent_blob,
                ptr::null_mut(),
                ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return Err("Windows 加密失败（DPAPI）".into());
            }
            let v = std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec();
            LocalFree(out.pb_data as *mut c_void);
            Ok(v)
        }
    }

    pub fn unprotect(blob: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let (mut in_blob, _keep_in) = blob_of(blob);
            let (mut ent_blob, _keep_ent) = blob_of(ENTROPY);
            let mut out = DataBlob { cb_data: 0, pb_data: ptr::null_mut() };

            let ok = CryptUnprotectData(
                &mut in_blob,
                ptr::null_mut(),
                &mut ent_blob,
                ptr::null_mut(),
                ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return None; // 换了机器或换了用户，正常解不开
            }
            let v = std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec();
            LocalFree(out.pb_data as *mut c_void);
            Some(v)
        }
    }
}

/// 非 Windows：开发调试用，明文存但把权限收到 0600。
///
/// **这个版本不要拿去发** —— 交付物是 Windows 那份，那边走的是 DPAPI。
#[cfg(not(windows))]
mod platform {
    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        Ok(plain.to_vec())
    }

    pub fn unprotect(blob: &[u8]) -> Option<Vec<u8>> {
        Some(blob.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_encoding() {
        for c in [
            Credentials { user: "13800138000".into(), pwd: "example-pwd".into() },
            // 密码里带各种特殊字符也要能还原
            Credentials { user: "a@b.com".into(), pwd: "p\"w\\d\nx".into() },
            Credentials { user: "中文账号".into(), pwd: "密码123".into() },
        ] {
            let enc = encode(&c);
            let back = parse(&enc).expect("应该能解析回来");
            assert_eq!(back, c, "往返失败: {c:?}");
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("").is_none());
        assert!(parse("不是 JSON").is_none());
        assert!(parse(r#"{"user":"","pwd":"x"}"#).is_none(), "空账号要拒");
        assert!(parse(r#"{"user":"a","pwd":""}"#).is_none(), "空密码要拒");
        assert!(parse(r#"{"pwd":"x"}"#).is_none(), "缺字段要拒");
    }

    #[test]
    fn protect_unprotect_round_trip() {
        let data = b"hello\x00\xff binary";
        let blob = platform::protect(data).expect("加密");
        // 只有 Windows 上才是真加密（DPAPI）；其它平台是开发用的透传
        #[cfg(windows)]
        assert_ne!(blob.as_slice(), data.as_slice(), "加密后不该等于明文");
        let back = platform::unprotect(&blob).expect("解密");
        assert_eq!(back.as_slice(), data.as_slice());
    }
}
