//! 本地 HTTP 服务：把 SDK 包成 REST，并托管内嵌的前端。
//!
//! 接口契约与前端 `web/src/lib/api.ts` 一一对应，字段名不能改。
//! MCP 工具和 HTTP 处理器共用这里的方法，所以业务逻辑都写成 `Server` 的方法，
//! 调用方只负责解析参数、调方法、把结果转成自己的格式。

use std::io::{self, Read};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

use crate::creds::Credentials;
use crate::lanzou::{str_of, Client, LzError, ROOT};

/// 蓝奏云免费账号的单文件上限。实测界限是闭区间：
/// 104857600 字节（100 M）能传，超 1 字节就报「无效文件」。
pub const MAX_UPLOAD_BYTES: u64 = 100 << 20;

/// `partsDesc` 是"这个文件夹里装的是一套分卷"的标记，写在文件夹描述里。
///
/// 为什么不靠查目录内容来判断：那样每列一次目录就要为每个文件夹多发一次请求，
/// 文件夹一多列表就会慢成串行。描述由列表接口直接返回，零成本。
pub const PARTS_DESC: &str = "分卷";

/// 分卷时每个块的大小。默认就是服务端上限（闭区间，用满不浪费），
/// 可以用 LANZOU_PART_BYTES 调小 —— 主要是为了方便用小文件测分卷逻辑。
pub fn part_size() -> u64 {
    std::env::var("LANZOU_PART_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        // 只能往小调：往上调没意义，服务端硬上限就在那儿，调大了每块都会被拒
        .map(|n| n.min(MAX_UPLOAD_BYTES))
        .unwrap_or(MAX_UPLOAD_BYTES)
}

/// 这么大要切成几块。
pub fn part_count(size: u64, ps: u64) -> usize {
    size.div_ceil(ps) as usize
}

/// 第 `idx` 块（从 1 开始）在文件里的起点和长度。
pub fn part_bounds(size: u64, ps: u64, idx: usize) -> (u64, u64) {
    let from = (idx as u64 - 1) * ps;
    (from, size.saturating_sub(from).min(ps))
}

/// 翻页上限，防止接口异常时无限翻页。
const MAX_PAGES: i64 = 50;
const MAX_SEARCH_DEPTH: usize = 3;
const MAX_SEARCH_DIRS: usize = 60;

// ---------- 对外的数据结构（字段名即 JSON 契约） ----------

#[derive(Serialize, Clone, Debug)]
pub struct FileItem {
    pub id: String,
    pub name: String,
    pub ext: String,
    pub bytes: i64,
    pub size: String,
    pub time: String,
    pub downs: i64,
    pub locked: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct FolderItem {
    pub id: String,
    pub name: String,
    pub des: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_parts: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct PathItem {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ListResponse {
    pub files: Vec<FileItem>,
    pub folders: Vec<FolderItem>,
    pub path: Vec<PathItem>,
    pub folder_id: i64,
    pub pages: i64,
    pub total: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct StatusResponse {
    pub uid: String,
    pub domain: String,
    pub domains: Vec<Value>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ShareResponse {
    pub f_id: String,
    pub pwd: String,
    pub locked: bool,
    pub url: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct UploadResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<FolderItem>,
    pub parts: usize,
}

/// 开始分卷上传的返回：目标文件夹 + 已经传上去的块号（断点续传用）。
#[derive(Serialize, Clone, Debug)]
pub struct BeginResponse {
    pub folder_id: String,
    pub folder_name: String,
    /// 已经在网盘上的块号（从 1 开始），前端跳过它们
    pub uploaded: Vec<usize>,
    pub parts: usize,
    /// 每块多大。前端按这个切，不用自己记 100M 这个数
    pub part_size: u64,
    /// 单个文件多大
    pub size: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct PartRef {
    pub id: String,
    pub name: String,
    pub idx: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchHit {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub folder_id: String,
    pub folder_path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub size: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub time: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_parts: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchResult {
    pub keyword: String,
    pub hits: Vec<SearchHit>,
    pub scanned_dirs: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

// ---------- 服务 ----------

pub struct Server {
    /// 当前配置的账号密码。None = 还没配置，界面该弹登录卡。
    creds: Mutex<Option<Credentials>>,
    /// 已经建立好的客户端。账号一换就丢掉重建。
    client: Mutex<Option<Arc<Client>>>,
    last_activity: AtomicI64,
}

impl Server {
    pub fn new() -> Arc<Self> {
        let s = Arc::new(Self {
            creds: Mutex::new(None),
            client: Mutex::new(None),
            last_activity: AtomicI64::new(now_millis()),
        });
        s.touch();
        s
    }

    /// 取当前客户端；没配置账号就报 NotConfigured（界面据此弹登录卡）。
    fn lz(&self) -> Result<Arc<Client>, LzError> {
        let mut slot = self.client.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = slot.as_ref() {
            return Ok(Arc::clone(c));
        }
        let creds = self.creds.lock().unwrap_or_else(|e| e.into_inner());
        let c = creds.as_ref().ok_or(LzError::NotConfigured)?;
        let client = Arc::new(Client::new(&c.user, &c.pwd)?);
        *slot = Some(Arc::clone(&client));
        Ok(client)
    }

    /// 启动时把磁盘上存的账号装进来（只记住，不登录 —— 登录交给第一次用的时候）。
    pub fn configure(&self, c: Credentials) {
        *self.creds.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
    }

    /// 当前配置的账号名（登录卡要预填）。没配置就是空串。
    pub fn current_user(&self) -> String {
        self.creds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|c| c.user.clone())
            .unwrap_or_default()
    }

    pub fn is_configured(&self) -> bool {
        self.creds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// 界面上登录：真去登录一次验证，成功了才记住。
    ///
    /// `remember` 为真时把账号密码加密存盘（下次开机免输）；为假只留在内存里，
    /// 这次会话有效，重启要重输。
    pub fn login_with(&self, user: &str, pwd: &str, remember: bool) -> Result<String, LzError> {
        if user.trim().is_empty() || pwd.is_empty() {
            return Err(LzError::Api("账号和密码都要填".into()));
        }
        // 真去登录一次 —— 不然用户打错密码，我们存了个错的，下次开机才发现
        let client = Client::new(user.trim(), pwd)?;
        let uid = client.uid()?;

        let c = Credentials {
            user: user.trim().to_string(),
            pwd: pwd.to_string(),
        };
        if remember {
            crate::creds::save(&c).map_err(LzError::Other)?;
        } else {
            crate::creds::clear();
        }
        *self.creds.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
        *self.client.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(client));
        Ok(uid)
    }

    /// 退出登录。`forget` 为真时把存盘凭据也抹掉（换账号走这条）。
    pub fn logout(&self, forget: bool) {
        *self.client.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.creds.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if forget {
            crate::creds::clear();
        }
        // 会话 cookie 一并清掉，否则下次启动还会拿旧会话去试
        let _ = std::fs::remove_file(crate::lanzou::cookie::cookie_path());
    }

    pub fn touch(&self) {
        self.last_activity.store(now_millis(), Ordering::Relaxed);
    }

    pub fn idle_millis(&self) -> i64 {
        now_millis() - self.last_activity.load(Ordering::Relaxed)
    }

    pub fn login(&self) -> Result<String, LzError> {
        self.lz()?.uid()
    }

    pub fn relogin(&self) -> Result<String, LzError> {
        self.lz()?.relogin()
    }

    /// 换出真实下载直链。HTTP 层和 MCP 工具都要用。
    pub fn direct_link(&self, file_id: &str) -> Result<String, LzError> {
        self.lz()?.direct_link(file_id)
    }

    // ---------- 读 ----------

    pub fn status(&self) -> Result<StatusResponse, LzError> {
        let uid = self.lz()?.uid()?;
        let domains = self.lz()?.domain_list()?;
        let first = domains
            .first()
            .map(|d| str_of(d, "domain").replace("https://", ""))
            .unwrap_or_default();
        Ok(StatusResponse {
            uid,
            domain: first,
            domains,
        })
    }

    /// 取目录内容。pg=0 表示自动翻页合并全部页，指定 pg 则只取该页。
    ///
    /// 蓝奏云 task=5 每页固定若干条，只取第一页会少一大截文件。
    pub fn list(&self, folder_id: i64, pg: i64) -> Result<ListResponse, LzError> {
        let mut files: Vec<FileItem> = Vec::new();
        let mut pages = 0i64;

        if pg > 0 {
            for raw in self.lz()?.files(folder_id, pg)? {
                files.push(to_file_item(&raw));
            }
            pages = 1;
        } else {
            for page in 1..=MAX_PAGES {
                let batch = self.lz()?.files(folder_id, page)?;
                if batch.is_empty() {
                    break;
                }
                for raw in &batch {
                    files.push(to_file_item(raw));
                }
                pages = page;
            }
        }

        let (fraw, praw) = self.lz()?.browse(folder_id)?;
        let folders = fraw.iter().map(to_folder_item).collect();
        let path = praw.iter().map(to_path_item).collect();

        Ok(ListResponse {
            total: files.len(),
            files,
            folders,
            path,
            folder_id,
            pages,
        })
    }

    pub fn share(&self, file_id: &str) -> Result<ShareResponse, LzError> {
        let info = self.lz()?.file_info(file_id)?;
        let short = str_of(&info, "f_id");
        let url = if short.is_empty() {
            String::new()
        } else {
            format!("{}/{}", str_of(&info, "is_newd"), short)
        };
        Ok(ShareResponse {
            f_id: short,
            pwd: str_of(&info, "pwd"),
            locked: str_of(&info, "onof") == "1",
            url,
        })
    }

    // ---------- 写 ----------

    pub fn mkdir(&self, name: &str, parent_id: i64) -> Result<FolderItem, LzError> {
        let safe = folder_safe(name);
        if safe.is_empty() {
            return Err(LzError::Other("文件夹名不能为空".into()));
        }
        let id = self.lz()?.new_folder(&safe, parent_for_write(parent_id), "")?;
        Ok(FolderItem {
            id,
            name: safe,
            des: String::new(),
            is_parts: false,
        })
    }


    pub fn delete_file(&self, file_id: &str) -> Result<Value, LzError> {
        self.lz()?.del_file(file_id)
    }

    /// task=3 对非空文件夹是递归删除（实测：文件夹和里面的文件一起没了，
    /// 不会留孤儿文件），所以不用先清空。
    pub fn delete_folder(&self, folder_id: &str) -> Result<Value, LzError> {
        if folder_id.parse::<i64>().is_err() {
            return Err(LzError::Other("文件夹 id 不是数字".into()));
        }
        self.lz()?.del_folder(folder_id)
    }

    // ---------- 上传 ----------

    /// 把一条字节流上传到 folder_id，超过单文件上限时自动建文件夹分卷。
    ///
    /// `size_hint` 是已知的总长度；< 0 表示未知，此时先读到上限多 1 字节再决定。
    pub fn upload<R: Read>(
        &self,
        mut reader: R,
        filename: &str,
        folder_id: i64,
        size_hint: i64,
    ) -> Result<UploadResponse, LzError> {
        let name = if filename.is_empty() { "unnamed" } else { filename };
        let ps = part_size();

        // 长度已知且超过单文件上限：直接走分卷，边读边传，内存里只留一块
        if size_hint > 0 && size_hint as u64 > ps {
            return self.upload_parts(reader, name, folder_id, size_hint as u64);
        }

        let mut body = Vec::new();
        reader
            .by_ref()
            .take(ps + 1)
            .read_to_end(&mut body)
            .map_err(|e| LzError::Http(e.to_string()))?;

        if body.is_empty() {
            return Err(LzError::Other("空文件".into()));
        }

        // 没声明 Content-Length 的请求：读到上限还溢出了，说明它其实是个大文件。
        // 分卷要先知道总长才能算块数，所以这里不硬吃进内存（700M 就得占 700M），
        // 让调用方带上长度、或者改用 /api/upload/begin 那套分块接口。
        if body.len() as u64 > ps {
            return Err(LzError::Other(
                "请求里没有长度，超过单文件上限的文件必须声明 Content-Length，或改用分块上传接口".into(),
            ));
        }

        let item = self.lz()?.upload_bytes(name, body, folder_id)?;
        Ok(UploadResponse {
            file: Some(to_file_item(&item)),
            folder: None,
            parts: 1,
        })
    }

    /// 分卷上传。和前端走同一套 begin / part / finish：
    /// 文件夹会复用、网盘上已有的块会跳过，所以中途失败再来一次是**续传**，
    /// 而不是从头再传一遍、还多出一个「名字_2」文件夹。
    ///
    fn upload_parts<R: Read>(
        &self,
        mut reader: R,
        filename: &str,
        folder_id: i64,
        size: u64,
    ) -> Result<UploadResponse, LzError> {
        let begin = self.upload_begin(filename, folder_id, size)?;

        for idx in 1..=begin.parts {
            let (_, want) = part_bounds(size, begin.part_size, idx);
            let want = want as usize;

            // 已经在网盘上的块：把这段读掉就行，不用再传一遍
            if begin.uploaded.contains(&idx) {
                io::copy(&mut reader.by_ref().take(want as u64), &mut io::sink())
                    .map_err(|e| LzError::Http(e.to_string()))?;
                continue;
            }

            let mut buf = vec![0u8; want];
            let n = read_full(&mut reader, &mut buf)?;
            if n != want {
                return Err(LzError::Other(format!(
                    "读第 {idx} 块时只读到 {n} 字节（应该有 {want}），文件被改动过？"
                )));
            }
            self.upload_part(&begin.folder_id, idx, filename, buf)?;
        }

        self.upload_finish(&begin.folder_id, filename, begin.parts)
    }

    /// 避免和已有文件夹重名 —— 重名会让两套分卷混在一起。
    fn unique_folder_name(&self, parent_id: i64, want: &str) -> Result<String, LzError> {
        let want = truncate_chars(&folder_safe(want), 80);
        let (folders, _) = self.lz()?.browse(parent_id)?;
        let used: Vec<String> = folders.iter().map(|f| str_of(f, "name")).collect();

        if !used.iter().any(|n| n == &want) {
            return Ok(want);
        }
        // 序号后缀用下划线：实测圆括号和空格都会被服务端拒，`_2` 不会
        for i in 2..200 {
            let cand = format!("{want}_{i}");
            if !used.iter().any(|n| n == &cand) {
                return Ok(cand);
            }
        }
        Err(LzError::Api(format!("同名文件夹太多了，请先清理「{want}」")))
    }

    // ---------- 分卷上传（前端切块，逐块传） ----------
    //
    // 以前是「一个请求传完整个大文件、服务端内部再切块」。700MB 那种要一个请求
    // 活三分钟以上，中间任何一层掐断都会让整个上传半途而废，而且前端拿不到
    // 真实进度。改成前端切块、一块一个请求之后：
    //   - 每个请求最多活几十秒，不会再有"活太久被掐"
    //   - 进度是真实的分块计数
    //   - 断点续传变成天然的：begin 会告诉前端哪些块已经在网盘上了

    /// 开始一次分卷上传。
    ///
    /// 会先找同名的分卷文件夹 —— **找到就复用**，并把已经传上去的块号返回给前端，
    /// 这样传一半断了重来时只补缺的那些块。
    pub fn upload_begin(
        &self,
        filename: &str,
        folder_id: i64,
        size: u64,
    ) -> Result<BeginResponse, LzError> {
        if size == 0 {
            return Err(LzError::Other("文件是空的".into()));
        }
        let part_size = part_size();
        let parts = part_count(size, part_size);
        let (base, ext) = split_name(filename);
        if ext.is_empty() {
            return Err(LzError::Other(
                "文件没有扩展名。蓝奏云不接受无名扩展的分块（会被当成 .001 拒绝），请先给文件加个扩展名".into(),
            ));
        }

        // 找同名且被标记为分卷的文件夹
        let (folders, _) = self.lz()?.browse(folder_id)?;
        let existing = folders
            .iter()
            .map(to_folder_item)
            .find(|f| f.is_parts && f.name == filename);

        let fol = match existing {
            Some(f) => f,
            None => {
                let name = self.unique_folder_name(folder_id, filename)?;
                let id = self
                    .lz()?
                    .new_folder(&name, parent_for_write(folder_id), PARTS_DESC)?;
                FolderItem {
                    id,
                    name,
                    des: PARTS_DESC.to_string(),
                    is_parts: true,
                }
            }
        };

        // 扫一遍这个文件夹，看哪些块已经在上面了
        let dest: i64 = fol
            .id
            .parse()
            .map_err(|_| LzError::Other(format!("文件夹 id 不是数字: {}", fol.id)))?;
        let re = part_regex();
        let mut uploaded: Vec<usize> = Vec::new();
        for page in 1..=MAX_PAGES {
            let batch = self.lz()?.files(dest, page)?;
            if batch.is_empty() {
                break;
            }
            for raw in &batch {
                if let Some(c) = re.captures(&file_name(raw)) {
                    if c[1] == base && c[3] == ext {
                        if let Ok(i) = c[2].parse::<usize>() {
                            uploaded.push(i);
                        }
                    }
                }
            }
        }
        uploaded.sort_unstable();

        Ok(BeginResponse {
            folder_id: fol.id,
            folder_name: fol.name,
            uploaded,
            parts,
            part_size,
            size,
        })
    }

    /// 传一块。`idx` 从 1 开始。
    pub fn upload_part(
        &self,
        folder_id: &str,
        idx: usize,
        filename: &str,
        data: Vec<u8>,
    ) -> Result<(), LzError> {
        let (base, ext) = split_name(filename);
        if ext.is_empty() {
            return Err(LzError::Other("文件没有扩展名".into()));
        }
        let dest: i64 = folder_id
            .parse()
            .map_err(|_| LzError::Other("文件夹 id 不是数字".into()))?;
        let want = part_name(&base, &ext, idx);

        // 蓝奏云碰到同名文件是「并存两份」而不是覆盖（实测），所以重传前得先清掉旧的
        // —— 不然一次失败重试就会留下重复块，合并时序号对不上，整个分卷作废。
        for page in 1..=MAX_PAGES {
            let batch = self.lz()?.files(dest, page)?;
            if batch.is_empty() {
                break;
            }
            for raw in &batch {
                if file_name(raw) == want {
                    let _ = self.lz()?.del_file(&str_of(raw, "id"));
                }
            }
        }

        // 上传接口不认 folder_id，所以先传根目录再移动进去
        let item = self.lz()?.upload_bytes(&want, data, ROOT)?;
        let fid = str_of(&item, "id");
        self.lz()?.move_file(&fid, dest)?;
        Ok(())
    }

    /// 收尾：核对块数齐了没有。
    pub fn upload_finish(
        &self,
        folder_id: &str,
        filename: &str,
        parts: usize,
    ) -> Result<UploadResponse, LzError> {
        // part_files 会校验序号连续，缺块会直接报错
        let refs = self.part_files(folder_id)?;
        if refs.len() < parts {
            return Err(LzError::Api(format!(
                "分卷不完整：应该有 {parts} 块，实际只有 {} 块，缺第 {} 块往后",
                refs.len(),
                refs.len() + 1
            )));
        }
        if refs.len() > parts {
            return Err(LzError::Api(format!(
                "分卷多出来 {} 块（应该是 {parts} 块），先清一下这个文件夹",
                refs.len() - parts
            )));
        }
        Ok(UploadResponse {
            file: None,
            folder: Some(FolderItem {
                id: folder_id.to_string(),
                name: filename.to_string(),
                des: PARTS_DESC.to_string(),
                is_parts: true,
            }),
            parts,
        })
    }

    // ---------- 分卷下载 ----------

    /// 把一个文件夹里的文件按分卷序号排好。
    ///
    /// 名字必须都长成 `<主干>.<序号><扩展名>`，且序号从 1 连续 —— 不满足就报错，
    /// 而不是硬拼出一个坏文件让用户白下载几百兆。
    pub fn part_files(&self, folder_id: &str) -> Result<Vec<PartRef>, LzError> {
        let id: i64 = folder_id
            .parse()
            .map_err(|_| LzError::Other("文件夹 id 不是数字".into()))?;

        let re = part_regex();
        let mut all: Vec<PartRef> = Vec::new();

        for page in 1..=MAX_PAGES {
            let batch = self.lz()?.files(id, page)?;
            if batch.is_empty() {
                break;
            }
            for raw in &batch {
                let name = file_name(raw);
                let caps = re
                    .captures(&name)
                    .ok_or_else(|| LzError::Api(format!("文件夹里有不符合分卷命名的文件「{name}」，无法合并")))?;
                let idx: i64 = caps[2]
                    .parse()
                    .map_err(|_| LzError::Api(format!("分卷序号解析不了: {name}")))?;
                all.push(PartRef {
                    id: str_of(raw, "id"),
                    name,
                    idx,
                });
            }
        }

        all.sort_by_key(|p| p.idx);
        for (i, p) in all.iter().enumerate() {
            if p.idx != i as i64 + 1 {
                return Err(LzError::Api(format!(
                    "分卷序号不连续（缺第 {} 块），合并出来会是坏文件",
                    i + 1
                )));
            }
        }
        Ok(all)
    }

    // ---------- 搜索 ----------

    pub fn search(&self, keyword: &str, folder_id: i64, deep: bool) -> Result<SearchResult, LzError> {
        let kw = keyword.trim().to_lowercase();
        if kw.is_empty() {
            return Err(LzError::Other("搜索关键词不能为空".into()));
        }

        let mut res = SearchResult {
            keyword: keyword.to_string(),
            hits: Vec::new(),
            scanned_dirs: 0,
            truncated: false,
        };

        let mut queue: Vec<(i64, String, usize)> = vec![(folder_id, String::new(), 0)];
        let mut head = 0usize;

        while head < queue.len() {
            if res.scanned_dirs >= MAX_SEARCH_DIRS {
                res.truncated = true;
                break;
            }
            let (cur_id, cur_path, depth) = queue[head].clone();
            head += 1;
            res.scanned_dirs += 1;

            let list = self.list(cur_id, 0)?;
            let label = if cur_path.is_empty() {
                "根目录".to_string()
            } else {
                cur_path
            };

            for f in &list.files {
                if f.name.to_lowercase().contains(&kw) {
                    res.hits.push(SearchHit {
                        kind: "file".into(),
                        id: f.id.clone(),
                        name: f.name.clone(),
                        folder_id: cur_id.to_string(),
                        folder_path: label.clone(),
                        size: f.size.clone(),
                        time: f.time.clone(),
                        is_parts: false,
                    });
                }
            }
            for fo in &list.folders {
                if fo.name.to_lowercase().contains(&kw) {
                    res.hits.push(SearchHit {
                        kind: "folder".into(),
                        id: fo.id.clone(),
                        name: fo.name.clone(),
                        folder_id: cur_id.to_string(),
                        folder_path: label.clone(),
                        size: String::new(),
                        time: String::new(),
                        is_parts: fo.is_parts,
                    });
                }
                if deep && depth < MAX_SEARCH_DEPTH {
                    if let Ok(id) = fo.id.parse::<i64>() {
                        queue.push((id, format!("{label} / {}", fo.name), depth + 1));
                    }
                }
            }
        }
        Ok(res)
    }
}

// ---------- 工具 ----------

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 读到 buf 满为止，返回实际读到的字节数（对齐 Go 的 io.ReadFull 语义）。
pub fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> Result<usize, LzError> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(LzError::Http(e.to_string())),
        }
    }
    Ok(n)
}

/// 拆出主干与扩展名（扩展名含点，没有则为空）。
/// 以点开头的名字（.gitignore）按"没有扩展名"处理。
pub fn split_name(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

/// 给第 idx 块起名（从 1 开始）：电影.zip → 电影.001.zip
///
/// 这个名字是实测过的：扩展名仍是原本的 .zip（在服务端白名单里），中间的点不影响。
/// 7-Zip 的 .001/.002 反而会被拒。
pub fn part_name(base: &str, ext: &str, idx: usize) -> String {
    format!("{base}.{idx:03}{ext}")
}

/// 新建文件夹时根目录要传 0，而列表接口用的是 -1。
pub fn parent_for_write(folder_id: i64) -> i64 {
    if folder_id == ROOT {
        0
    } else {
        folder_id
    }
}

/// 实测被蓝奏云拒掉的字符（报「名称含有特殊字符」）。
///
/// 文件夹名的限制比文件名严得多：文件名可以是「我的 电影.zip」，
/// 但同样的名字做文件夹名会被拒。实测半角空格、圆括号、`# + , ' % $ = ; : * ? " < |`
/// 都不行，全角空格 U+3000 也不行；而 `_ - . [ ] ~ ! @ &` 和中文都正常。
const FOLDER_UNSAFE: &str = " ()#+,;:=%$*?\"<>|'\\/{}\u{5e}`\t\n\r\u{3000}";

pub fn folder_safe(name: &str) -> String {
    name.chars()
        .map(|c| if FOLDER_UNSAFE.contains(c) { '_' } else { c })
        .collect()
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn part_regex() -> regex::Regex {
    regex::Regex::new(r"^(.*)\.(\d{3,})(\.[^.]+)$").expect("常量正则")
}

fn size_regex() -> regex::Regex {
    regex::Regex::new(r"(?i)([\d.]+)\s*([BKMG])").expect("常量正则")
}

/// 把 "37.4 K" / "27.1 M" / "17.0 B" 反推成字节数（**估算值**）。
///
/// 蓝奏云列表只给人类可读的舍入字符串，一位小数意味着约 ±0.05 单位的误差，
/// 精确长度只有下载响应的 Content-Length 才给得出，别拿这个做完整性校验。
pub fn parse_size(text: &str) -> i64 {
    let re = size_regex();
    let caps = match re.captures(text) {
        Some(c) => c,
        None => return 0,
    };
    let f: f64 = caps[1].parse().unwrap_or(0.0);
    let unit = match caps[2].to_uppercase().as_str() {
        "B" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        _ => return 0,
    };
    (f * unit) as i64
}

/// 文件列表里的名字：优先 name_all（带扩展名），退回 name。
fn file_name(raw: &Value) -> String {
    let a = str_of(raw, "name_all");
    if a.is_empty() {
        str_of(raw, "name")
    } else {
        a
    }
}

pub fn to_file_item(raw: &Value) -> FileItem {
    let name = file_name(raw);
    let mut ext = str_of(raw, "icon").to_lowercase();
    if ext.is_empty() || ext == "file" {
        ext = match name.rfind('.') {
            Some(i) => name[i + 1..].to_lowercase(),
            None => "file".to_string(),
        };
    }
    FileItem {
        id: str_of(raw, "id"),
        name,
        ext,
        bytes: parse_size(&str_of(raw, "size")),
        size: str_of(raw, "size"),
        time: str_of(raw, "time"),
        downs: crate::lanzou::convert::int_of(raw, "downs"),
        locked: str_of(raw, "onof") == "1",
    }
}

/// task=47 的 text[]（子文件夹），主键字段是 fol_id。
pub fn to_folder_item(raw: &Value) -> FolderItem {
    let mut des = {
        let a = str_of(raw, "folder_des");
        if a.is_empty() {
            str_of(raw, "des")
        } else {
            a
        }
    };
    // 列表接口把描述包在方括号里返回（"[分卷]"），详情接口才是干净的
    des = des.trim_start_matches('[').trim_end_matches(']').to_string();

    let id = {
        let a = str_of(raw, "fol_id");
        if a.is_empty() {
            str_of(raw, "folderid")
        } else {
            a
        }
    };
    FolderItem {
        id,
        name: str_of(raw, "name"),
        is_parts: des.starts_with(PARTS_DESC),
        des,
    }
}

/// task=47 的 info[]（面包屑），主键字段是 folderid。
pub fn to_path_item(raw: &Value) -> PathItem {
    let id = {
        let a = str_of(raw, "folderid");
        if a.is_empty() {
            str_of(raw, "fol_id")
        } else {
            a
        }
    };
    PathItem {
        id,
        name: str_of(raw, "name"),
    }
}

/// 按 RFC 5987 编码文件名，供 Content-Disposition 的 filename* 使用。
pub fn rfc5987(name: &str) -> String {
    percent_encoding::utf8_percent_encode(name, RFC5987).to_string()
}

use percent_encoding::{AsciiSet, CONTROLS};

/// 在 query 编码的基础上把 `'` 也编掉（RFC 5987 要求），空格用 %20。
const RFC5987: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'%')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b';')
    .add(b'=')
    .add(b':')
    .add(b'@')
    .add(b'[')
    .add(b']');

/// 把 JSON 错误体写成统一形状。
pub fn error_body(msg: &str) -> String {
    json!({ "detail": msg }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_part_name() {
        let cases = [
            ("电影.zip", ("电影", ".zip"), "电影.001.zip"),
            ("movie.mp4", ("movie", ".mp4"), "movie.001.mp4"),
            ("archive.tar.gz", ("archive.tar", ".gz"), "archive.tar.001.gz"),
            ("noext", ("noext", ""), ""),
            (".gitignore", (".gitignore", ""), ""),
            ("a.b.c.7z", ("a.b.c", ".7z"), "a.b.c.001.7z"),
        ];
        for (input, want_split, want_part) in cases {
            let (b, e) = split_name(input);
            assert_eq!((b.as_str(), e.as_str()), want_split, "split_name({input})");
            if !e.is_empty() {
                assert_eq!(part_name(&b, &e, 1), want_part);
            }
        }
    }

    #[test]
    fn part_math_handles_boundaries() {
        // 上限是闭区间：正好 100 M 是一块，多 1 字节就得两块
        assert_eq!(part_count(100 << 20, 100 << 20), 1);
        assert_eq!(part_count((100 << 20) + 1, 100 << 20), 2);
        // 800 M 那次实测就是 8 块
        assert_eq!(part_count(838_860_800, 104_857_600), 8);

        let (from, len) = part_bounds(838_860_800, 104_857_600, 1);
        assert_eq!((from, len), (0, 104_857_600));
        let (from, len) = part_bounds(838_860_800, 104_857_600, 8);
        assert_eq!((from, len), (734_003_200, 104_857_600));

        // 最后一块通常是不满的
        assert_eq!(part_bounds(12_000_000, 5_000_000, 3), (10_000_000, 2_000_000));
        assert_eq!(part_bounds(5, 5, 1), (0, 5));
    }

    #[test]
    fn part_regex_matches_expected() {
        let re = part_regex();
        for good in ["电影.001.zip", "a.002.7z", "x.y.010.rar"] {
            assert!(re.is_match(good), "{good} 应该被认成分卷");
        }
        for bad in ["电影.zip", "电影.001", "电影.1.zip", "readme.txt"] {
            assert!(!re.is_match(bad), "{bad} 不该被认成分卷");
        }
    }

    #[test]
    fn folder_safe_keeps_cjk_and_fullwidth() {
        let cases = [
            ("电影.zip", "电影.zip"),
            ("我的 电影.zip", "我的_电影.zip"),
            ("a(1).zip", "a_1_.zip"),
            ("a（1）.zip", "a（1）.zip"),          // 全角括号服务端收，保留
            ("视频，合集.zip", "视频，合集.zip"),     // 中文标点保留
            ("probe_part[2].zip", "probe_part[2].zip"),
            ("a~b!c@d&e.zip", "a~b!c@d&e.zip"),
            ("full\u{3000}width.zip", "full_width.zip"),
        ];
        for (input, want) in cases {
            assert_eq!(folder_safe(input), want, "folder_safe({input})");
        }
    }

    #[test]
    fn folder_safe_keeps_extension() {
        for input in ["我的 电影.zip", "a b c.7z", "x(y).rar"] {
            let (base, ext) = split_name(&folder_safe(input));
            assert!(!base.is_empty() && !ext.is_empty(), "{input} 扩展名被破坏了");
        }
    }

    #[test]
    fn folder_item_detects_parts() {
        let raw = json!({"fol_id": "14045213", "name": "电影.zip", "folder_des": "[分卷]"});
        let it = to_folder_item(&raw);
        assert!(it.is_parts);
        assert_eq!(it.des, "分卷");

        let plain = json!({"fol_id": "1", "name": "论文存储"});
        assert!(!to_folder_item(&plain).is_parts);
    }

    #[test]
    fn parse_size_variants() {
        assert_eq!(parse_size("37.4 K"), 38297);
        assert_eq!(parse_size("100.0 M"), 104857600);
        assert_eq!(parse_size("17.0 B"), 17);
        assert_eq!(parse_size(""), 0);
        assert_eq!(parse_size("乱七八糟"), 0);
    }
}
