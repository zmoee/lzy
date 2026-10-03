//! 阿里云 ESA `acw_sc__v2` 挑战算法 —— 蓝奏云全站的前置风控。
//!
//! 挑战页里带一段 `var arg1='<40 位十六进制>'`，把 arg1 按固定置换表重排，
//! 再与固定 key 逐字节异或，得到的 40 位十六进制串就是 cookie 的值。
//! 置换表和 key 都是从挑战脚本里逆向出来的常量。
//!
//! 注意三个域名（.woozooo.com / *.lanzn.com / *.lanrar.com）各有一套挑战，
//! cookie 按 host 隔离，互不通用。

/// 置换表：重排后的第 z 位来自 arg1 的第 PERM[z]-1 位。
const PERM: [u8; 40] = [
    0xf, 0x23, 0x1d, 0x18, 0x21, 0x10, 0x1, 0x26, 0xa, 0x9, 0x13, 0x1f, 0x28, 0x1b, 0x16, 0x17,
    0x19, 0xd, 0x6, 0xb, 0x27, 0x12, 0x14, 0x8, 0xe, 0x15, 0x20, 0x1a, 0x2, 0x1e, 0x7, 0x4, 0x11,
    0x5, 0x3, 0x1c, 0x22, 0x25, 0xc, 0x24,
];

const KEY: &str = "3000176000856006061501533003690027800375";

#[derive(Debug)]
pub enum AcwError {
    /// 这段 HTML 不是挑战页（正常页面，或者挑战页结构变了）
    NotAChallenge,
    BadArg1(String),
}

impl std::fmt::Display for AcwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcwError::NotAChallenge => write!(f, "页面里没有 arg1，不是挑战页"),
            AcwError::BadArg1(s) => write!(f, "arg1 无法解析: {s}"),
        }
    }
}

impl std::error::Error for AcwError {}

/// 从挑战页 HTML 里取出 arg1 并算出 cookie 值。
pub fn solve_html(html: &str) -> Result<String, AcwError> {
    let arg1 = extract_arg1(html).ok_or(AcwError::NotAChallenge)?;
    solve_arg1(&arg1)
}

/// 找出 `arg1='...'` 里的内容，不引正则 —— 就为这一处不值得。
fn extract_arg1(html: &str) -> Option<String> {
    let at = html.find("arg1='")? + "arg1='".len();
    let rest = &html[at..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

/// 对已知的 arg1 求值。
pub fn solve_arg1(arg1: &str) -> Result<String, AcwError> {
    if arg1.len() != PERM.len() {
        return Err(AcwError::BadArg1(format!(
            "长度 {} 不是 {}",
            arg1.len(),
            PERM.len()
        )));
    }
    if !arg1.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AcwError::BadArg1("含非十六进制字符".into()));
    }

    // 重排：out[z] = arg1[PERM[z]-1]
    let src = arg1.as_bytes();
    let mut rearranged = String::with_capacity(PERM.len());
    for &pos in PERM.iter() {
        let idx = pos as usize - 1;
        rearranged.push(src[idx] as char);
    }

    // 逐字节异或：重排结果与 key 各按十六进制解成字节
    let a = hex_decode(&rearranged)?;
    let k = hex_decode(KEY)?;
    let n = a.len().min(k.len());
    let mut out = String::with_capacity(n * 2);
    for i in 0..n {
        out.push_str(&format!("{:02x}", a[i] ^ k[i]));
    }
    Ok(out)
}

fn hex_decode(s: &str) -> Result<Vec<u8>, AcwError> {
    if !s.len().is_multiple_of(2) {
        return Err(AcwError::BadArg1(format!("十六进制串长度是奇数: {}", s.len())));
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..b.len()).step_by(2) {
        let hi = (b[i] as char)
            .to_digit(16)
            .ok_or_else(|| AcwError::BadArg1("非法十六进制字符".into()))?;
        let lo = (b[i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| AcwError::BadArg1("非法十六进制字符".into()))?;
        out.push(((hi << 4) | lo) as u8);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 Go / Python 版逐位对拍。置换表和 key 抄错一位就全盘失效，
    /// 而失效的表现是"所有请求都命中挑战"，非常难查 —— 所以做穷尽比对。
    #[test]
    fn matches_reference_implementation() {
        let data = std::fs::read_to_string("testdata/vectors.txt").expect("读不到向量文件");
        let mut count = 0;
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split_whitespace();
            let (arg1, want) = (it.next().unwrap(), it.next().unwrap());
            let got = solve_arg1(arg1).unwrap_or_else(|e| panic!("{arg1} 求解失败: {e}"));
            assert_eq!(got, want, "arg1={arg1}");
            count += 1;
        }
        assert!(count >= 500, "只读到 {count} 组向量，文件可能不完整");
        eprintln!("与参考实现逐位一致，共 {count} 组");
    }

    #[test]
    fn parses_challenge_page() {
        let page = r#"<html><script>
            var arg1='ADBE45923C0FF056568B734FA473BD93ADAF90F1';
            var _0x4f2a=['\x61\x63\x77'];
        </script></html>"#;
        assert_eq!(
            solve_html(page).unwrap(),
            "6abfb1c0c30c7732a945f7e13737b49e7333da8a"
        );
    }

    #[test]
    fn rejects_non_challenge() {
        for page in ["<html>正常页面</html>", "", "arg1=", "arg1='xyz'", "ARG1='ADBE'"] {
            assert!(solve_html(page).is_err(), "{page:?} 不该被当成挑战页");
        }
    }

    #[test]
    fn guards_arg1_length() {
        for bad in ["", "ABCD", &"A".repeat(39), &"A".repeat(41)] {
            assert!(solve_arg1(bad).is_err());
        }
    }

    #[test]
    fn perm_is_a_permutation_of_1_to_40() {
        let mut seen = [false; 41];
        for &p in PERM.iter() {
            assert!((1..=40).contains(&p), "置换表取值越界: {p}");
            assert!(!seen[p as usize], "置换表有重复值: {p}");
            seen[p as usize] = true;
        }
        assert!(seen[1..].iter().all(|&b| b));
    }
}
