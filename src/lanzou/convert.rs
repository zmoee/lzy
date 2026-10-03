//! 蓝奏云响应的取值助手。
//!
//! 服务端的字段类型很松散：`zt` 可能是数字 `1`、字符串 `"1"` 或 `null`；
//! `id` 是数字；`downs` 有时是数字有时是字符串。统一按字符串取，
//! 免得数字被格式化成一坨。

use serde_json::Value;

/// 取业务状态码的字符串形式；`null` / 缺省都返回空串。
///
/// - `"1"`  成功
/// - `"0"`  业务失败，`info` 是中文原因
/// - `"2"`  特殊分支（task47 表示"已进入某目录"；41/39 表示会员限制）
/// - `"9"`  会话失效
/// - `""`   会员校验失败时返回的 null
pub fn zt_of(v: &Value) -> String {
    str_of(v, "zt")
}

/// 把一个 JSON 值当字符串取。数字按整数形式输出（不会变成 `3.19725286e8`）。
pub fn value_str(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else if let Some(f) = n.as_f64() {
                // 整数形式的浮点（JSON 里 123.0）不要带小数点
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{}", f as i64)
                } else {
                    f.to_string()
                }
            } else {
                n.to_string()
            }
        }
        _ => String::new(),
    }
}

pub fn str_of(obj: &Value, key: &str) -> String {
    obj.get(key).map(value_str).unwrap_or_default()
}

pub fn int_of(obj: &Value, key: &str) -> i64 {
    let s = str_of(obj, key);
    s.trim()
        .parse::<f64>()
        .map(|f| f as i64)
        .unwrap_or_default()
}

/// 取对象数组字段；字段不存在或类型不对时返回空数组。
pub fn list_of(obj: &Value, key: &str) -> Vec<Value> {
    match obj.get(key) {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

pub fn first_non_empty(vals: &[String]) -> String {
    vals.iter().find(|s| !s.is_empty()).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_stay_integer_shaped() {
        // 这是 Go 版踩过的坑：float64 会把 319725286 变成 "3.19725286e+08"
        let v = json!({ "id": 319725286 });
        assert_eq!(str_of(&v, "id"), "319725286");

        let v2 = json!({ "id": "319725286" });
        assert_eq!(str_of(&v2, "id"), "319725286");

        let v3 = json!({ "id": 319725286.0 });
        assert_eq!(str_of(&v3, "id"), "319725286");
    }

    #[test]
    fn zt_variants() {
        assert_eq!(zt_of(&json!({"zt": 1})), "1");
        assert_eq!(zt_of(&json!({"zt": "1"})), "1");
        assert_eq!(zt_of(&json!({"zt": 2})), "2");
        assert_eq!(zt_of(&json!({"zt": null})), "");
        assert_eq!(zt_of(&json!({})), "");
    }

    #[test]
    fn missing_and_null_are_empty() {
        let v = json!({ "a": null });
        assert_eq!(str_of(&v, "a"), "");
        assert_eq!(str_of(&v, "nope"), "");
        assert_eq!(int_of(&v, "nope"), 0);
        assert!(list_of(&v, "nope").is_empty());
    }

    #[test]
    fn list_of_rejects_wrong_type() {
        let v = json!({ "text": "不是数组" });
        assert!(list_of(&v, "text").is_empty());
        let v2 = json!({ "text": [{"id": 1}, {"id": 2}] });
        assert_eq!(list_of(&v2, "text").len(), 2);
    }
}
