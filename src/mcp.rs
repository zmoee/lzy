//! MCP 服务（Model Context Protocol over stdio）。
//!
//! 让 AI 直接操作这个网盘：列目录、搜索、上传、下载、分享、建目录、删除。
//!
//! 传输是 JSON-RPC 2.0，一行一条消息。请求走 stdin，响应走 stdout ——
//! 所以**这里绝不能往 stdout 打别的任何东西**（日志、进度都不行），
//! 那会破坏协议帧。所有提示走 stderr。
//!
//! 另一个注意点：Windows 上这个程序是 GUI 子系统，没有控制台。
//! 被 MCP 客户端拉起来时 stdin/stdout 是客户端给的管道，能用；
//! 但要是有人手敲 `lzy --mcp`，那两个句柄可能是空的 ——
//! 所以这里一律用"写失败就算了"的方式，不用 println!（它在写不出去时会 panic）。

use std::io::{BufRead, Write};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::lanzou::{str_of, ROOT};
use crate::ops;
use crate::server::Server;

/// 支持并愿意协商的协议版本。客户端报的版本如果在这里面就顺着它，
/// 否则回我们自己的最新版（客户端可以据此决定要不要断）。
const SUPPORTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
const LATEST_VERSION: &str = "2025-06-18";

const SERVER_NAME: &str = "lzy";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn serve(app: Arc<Server>) -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break, // 管道断了，客户端没了
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                write_msg(&mut out, &jsonrpc_error(Value::Null, -32700, &format!("JSON 解析失败: {e}")));
                continue;
            }
        };

        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // 没有 id 的是通知，按规范不回响应
        let id = match msg.get("id") {
            Some(v) if !v.is_null() => v.clone(),
            _ => {
                // notifications/initialized、notifications/cancelled 等，忽略即可
                let _ = method;
                continue;
            }
        };

        let resp = match method {
            "initialize" => jsonrpc_ok(id.clone(), initialize_result(&params)),
            "tools/list" => jsonrpc_ok(id.clone(), json!({ "tools": tool_defs() })),
            "tools/call" => jsonrpc_ok(id.clone(), call_tool(&app, &params)),
            "ping" => jsonrpc_ok(id.clone(), json!({})),
            other => jsonrpc_error(id.clone(), -32601, &format!("不认识的方法: {other}")),
        };
        write_msg(&mut out, &resp);
    }
    0
}

fn write_msg(out: &mut impl Write, v: &Value) {
    // 协议帧就是一行 JSON。写不出去（stdout 没接上）只能算了，不能 panic。
    let _ = writeln!(out, "{}", v);
    let _ = out.flush();
}

fn jsonrpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn jsonrpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize_result(params: &Value) -> Value {
    let asked = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let negotiated = if SUPPORTED_VERSIONS.contains(&asked) {
        asked
    } else {
        LATEST_VERSION
    };
    json!({
        "protocolVersion": negotiated,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
    })
}

// ---------- 工具的返回形状 ----------

fn text_result(body: String) -> Value {
    json!({ "content": [{ "type": "text", "text": body }] })
}

fn error_result(msg: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg.into() }],
        "isError": true,
    })
}

fn ok_json<T: serde::Serialize>(v: &T) -> Value {
    match serde_json::to_string_pretty(v) {
        Ok(s) => text_result(s),
        Err(e) => error_result(format!("结果序列化失败: {e}")),
    }
}

// ---------- 参数取值 ----------

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn arg_i64(args: &Value, key: &str, def: i64) -> i64 {
    match args.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(def),
        Some(Value::String(s)) => s.parse().unwrap_or(def),
        _ => def,
    }
}

fn arg_bool(args: &Value, key: &str, def: bool) -> bool {
    match args.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => def,
    }
}

fn arg_str_array(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

// ---------- 工具定义 ----------
//
// 描述写详细一点是值得的：AI 全靠这段文字判断怎么调、参数是什么含义。

fn tool_defs() -> Value {
    json!([
        {
            "name": "lzy_status",
            "description": "看蓝奏云账号状态：uid、主分享域名。用它确认能不能连上。",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "lzy_list",
            "description": "列出网盘某个目录的内容（文件 + 子文件夹 + 面包屑路径）。根目录的 folder_id 是 -1，省略也默认根目录。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "folder_id": { "type": "integer", "description": "要列的目录 id，根目录用 -1（默认）" }
                }
            }
        },
        {
            "name": "lzy_find",
            "description": "按名字搜文件/文件夹。默认只搜根目录；文件可能放在子文件夹里时加 deep=true 递归。递归有深度和数量上限，结果里的 truncated=true 表示没搜完。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keyword": { "type": "string", "description": "要匹配的关键词，不区分大小写" },
                    "deep": { "type": "boolean", "description": "是否递归进子文件夹，默认 false" }
                },
                "required": ["keyword"]
            }
        },
        {
            "name": "lzy_upload",
            "description": "把本机文件传到网盘。传的是本地路径（不是文件内容）。超过 100 M 的文件会被自动切成多块、放进一个同名文件夹里，返回的 parts 会大于 1。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "要上传的本地文件绝对路径，可以给多个"
                    },
                    "folder_id": { "type": "integer", "description": "传到哪个目录，根目录用 -1（默认）" }
                },
                "required": ["paths"]
            }
        },
        {
            "name": "lzy_download",
            "description": "把网盘上的文件下到本机。注意有两种目标：普通文件用 id + save_to；如果那个 id 是「分卷文件夹」（list 里带 is_parts:true），必须加 folder=true，工具会把各块自动拼回原文件。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "文件 id；folder=true 时是文件夹 id" },
                    "save_to": { "type": "string", "description": "保存到本机的绝对路径" },
                    "folder": { "type": "boolean", "description": "id 是分卷文件夹时传 true，会自动合并还原" }
                },
                "required": ["id", "save_to"]
            }
        },
        {
            "name": "lzy_share",
            "description": "拿一个文件的分享链接和提取码。返回里 locked=true 表示需要提取码，pwd 就是那串码 —— **两个都要给用户**。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file_id": { "type": "string", "description": "文件 id" }
                },
                "required": ["file_id"]
            }
        },
        {
            "name": "lzy_mkdir",
            "description": "在网盘上新建文件夹。名字里的空格和半角括号会被自动换成下划线（蓝奏云不接受这些字符），返回的 name 才是真正建出来的名字。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "文件夹名" },
                    "parent_id": { "type": "integer", "description": "建在哪个目录下，根目录用 -1（默认）" }
                },
                "required": ["name"]
            }
        },
        {
            "name": "lzy_delete",
            "description": "删除文件或文件夹。进回收站，可以从网页版还原，不是永久删除。删文件夹会连同里面的内容一起删。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "文件 id 或文件夹 id" },
                    "kind": {
                        "type": "string",
                        "enum": ["file", "folder"],
                        "description": "默认是 file；删文件夹必须显式传 folder"
                    }
                },
                "required": ["id"]
            }
        }
    ])
}

// ---------- 工具实现 ----------

fn call_tool(app: &Arc<Server>, params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    match name {
        "lzy_status" => match app.status() {
            Ok(v) => ok_json(&v),
            Err(e) => error_result(e.to_string()),
        },

        "lzy_list" => {
            let folder_id = arg_i64(&args, "folder_id", ROOT);
            match app.list(folder_id, 0) {
                Ok(v) => ok_json(&v),
                Err(e) => error_result(e.to_string()),
            }
        }

        "lzy_find" => {
            let kw = match arg_str(&args, "keyword") {
                Some(k) if !k.trim().is_empty() => k,
                _ => return error_result("缺少 keyword"),
            };
            match app.search(&kw, ROOT, arg_bool(&args, "deep", false)) {
                Ok(v) => ok_json(&v),
                Err(e) => error_result(e.to_string()),
            }
        }

        "lzy_upload" => {
            let paths = arg_str_array(&args, "paths");
            if paths.is_empty() {
                return error_result("缺少 paths（要上传的本地文件路径）");
            }
            let folder_id = arg_i64(&args, "folder_id", ROOT);
            let res = ops::upload_paths(app, &paths, folder_id);
            let mut out = ok_json(&res);
            // 部分失败要标成错误，不然调用方会以为全成功了
            if !res.failed.is_empty() {
                if let Some(o) = out.as_object_mut() {
                    o.insert("isError".into(), json!(true));
                }
            }
            out
        }

        "lzy_download" => {
            let id = match arg_str(&args, "id") {
                Some(v) => v,
                None => return error_result("缺少 id"),
            };
            let dest = match arg_str(&args, "save_to") {
                Some(v) => v,
                None => return error_result("缺少 save_to（保存到本机的路径）"),
            };
            let folder = arg_bool(&args, "folder", false);
            match ops::download(app, &id, &dest, folder) {
                Ok(n) => ok_json(&json!({ "saved_to": dest, "bytes": n })),
                Err(e) => error_result(e),
            }
        }

        "lzy_share" => {
            let id = match arg_str(&args, "file_id") {
                Some(v) => v,
                None => return error_result("缺少 file_id"),
            };
            match app.share(&id) {
                Ok(v) => ok_json(&v),
                Err(e) => error_result(e.to_string()),
            }
        }

        "lzy_mkdir" => {
            let name = match arg_str(&args, "name") {
                Some(v) if !v.trim().is_empty() => v,
                _ => return error_result("缺少 name"),
            };
            let parent = arg_i64(&args, "parent_id", ROOT);
            match app.mkdir(&name, parent) {
                Ok(v) => ok_json(&v),
                Err(e) => error_result(e.to_string()),
            }
        }

        "lzy_delete" => {
            let id = match arg_str(&args, "id") {
                Some(v) => v,
                None => return error_result("缺少 id"),
            };
            let is_folder = arg_str(&args, "kind").as_deref() == Some("folder");
            let r = if is_folder {
                app.delete_folder(&id)
            } else {
                app.delete_file(&id)
            };
            match r {
                Ok(v) => ok_json(&v),
                Err(e) => error_result(e.to_string()),
            }
        }

        other => error_result(format!("不认识的工具: {other}")),
    }
}

/// 让 `str_of` 在这个模块里可见（lanzou 的松散取值助手，测试里会用到）。
#[allow(dead_code)]
fn _unused(v: &Value) -> String {
    str_of(v, "zt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_negotiates_version() {
        // 客户端报一个我们支持的版本 → 顺着它
        let r = initialize_result(&json!({ "protocolVersion": "2025-03-26" }));
        assert_eq!(r["protocolVersion"], "2025-03-26");

        // 报一个不认识的 → 回我们的最新版
        let r2 = initialize_result(&json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(r2["protocolVersion"], LATEST_VERSION);

        // 压根没报 → 也是最新版
        let r3 = initialize_result(&json!({}));
        assert_eq!(r3["protocolVersion"], LATEST_VERSION);
    }

    #[test]
    fn tools_all_have_schema_and_description() {
        let tools = tool_defs();
        let arr = tools.as_array().expect("工具列表应是数组");
        assert!(arr.len() >= 8, "工具太少: {}", arr.len());
        for t in arr {
            let name = t["name"].as_str().unwrap_or("");
            assert!(name.starts_with("lzy_"), "工具名要带前缀: {name}");
            let desc = t["description"].as_str().unwrap_or("");
            assert!(desc.len() > 20, "{name} 的描述太短，AI 没法判断怎么用");
            assert_eq!(t["inputSchema"]["type"], "object", "{name} 的 schema 要是 object");
        }
    }

    #[test]
    fn argument_helpers_are_forgiving() {
        let a = json!({ "folder_id": -1, "deep": true, "paths": ["a.zip", "b.zip"] });
        assert_eq!(arg_i64(&a, "folder_id", 0), -1);
        assert!(arg_bool(&a, "deep", false));
        assert_eq!(arg_str_array(&a, "paths").len(), 2);

        // 字符串形式的数字也要认（有些客户端会这么传）
        let b = json!({ "folder_id": "-1", "deep": "true" });
        assert_eq!(arg_i64(&b, "folder_id", 0), -1);
        assert!(arg_bool(&b, "deep", false));

        // 缺字段走默认
        let c = json!({});
        assert_eq!(arg_i64(&c, "folder_id", -1), -1);
        assert!(!arg_bool(&c, "deep", false));
        assert!(arg_str_array(&c, "paths").is_empty());
    }

    #[test]
    fn error_result_marks_is_error() {
        let e = error_result("炸了");
        assert_eq!(e["isError"], true);
        assert_eq!(e["content"][0]["text"], "炸了");

        // 成功的结果不该带 isError
        let ok = text_result("好的".into());
        assert!(ok.get("isError").is_none());
    }

    #[test]
    fn jsonrpc_shapes() {
        let ok = jsonrpc_ok(json!(1), json!({"a":1}));
        assert_eq!(ok["jsonrpc"], "2.0");
        assert_eq!(ok["id"], 1);
        assert!(ok.get("error").is_none());

        let err = jsonrpc_error(json!(2), -32601, "不认识");
        assert_eq!(err["error"]["code"], -32601);
        assert!(err.get("result").is_none());
    }
}
