// MiMo 平台数据层（从 data.js 移植）
// 凭证读取顺序：xiaomi-account 分区 Cookies SQLite → 分区 HTTP 缓存里缓存的 serviceLogin 响应
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub const API: &str = "https://platform.xiaomimimo.com/api/v1";
const SUB_BASE: &str = "https://mimo-server-cn.xiaomimimo.com/api";

fn ua() -> &'static str {
    if cfg!(target_os = "windows") {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36"
    } else if cfg!(target_os = "macos") {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
    } else {
        "Mozilla/5.0 (X11; Linux x86_64)"
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ---------- MiMo Desktop 凭证 ----------

fn desktop_user_data() -> PathBuf {
    if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join("AppData").join("Roaming"));
        appdata.join("Xiaomi MiMo")
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().unwrap_or_default().join("Library").join("Application Support").join("Xiaomi MiMo")
    } else {
        dirs::home_dir().unwrap_or_default().join(".config").join("Xiaomi MiMo")
    }
}

fn cookie_db_candidates() -> Vec<PathBuf> {
    let partition = desktop_user_data().join("Partitions").join("xiaomi-account");
    vec![partition.join("Network").join("Cookies"), partition.join("Cookies")]
}

#[derive(Clone, Debug)]
pub struct Creds {
    pub pass_token: String,
    pub user_id: String,
    pub c_user_id: Option<String>,
    pub mtime: f64,
}

// 服务端拒绝过的凭证：登录时逐枚换候选，避免死磕同一枚坏 token
static REJECTED: LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

fn rejected_contains(tok: &str) -> bool {
    REJECTED.lock().unwrap().contains(tok)
}

fn rejected_insert(tok: String) {
    REJECTED.lock().unwrap().insert(tok);
}

fn read_cookie_db(db_path: &Path) -> Result<HashMap<String, String>, String> {
    if !db_path.exists() {
        return Err(format!("cookie db missing: {}", db_path.display()));
    }
    // 平台登录与订阅登录可能并发：pid+单调计数保证 temp 目录唯一
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "mimocookie_{}_{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        let name = db_path.file_name().and_then(|n| n.to_str()).unwrap_or("Cookies");
        for suffix in ["", "-wal", "-shm"] {
            let src = db_path.with_file_name(format!("{}{}", name, suffix));
            if src.exists() {
                std::fs::copy(&src, tmp.join(format!("{}{}", name, suffix))).map_err(|e| e.to_string())?;
            }
        }
        let conn = rusqlite::Connection::open(tmp.join(name)).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare("select name, value from cookies").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut map = HashMap::new();
        for r in rows {
            let (k, v) = r.map_err(|e| e.to_string())?;
            map.insert(k, v);
        }
        if !map.contains_key("passToken") || !map.contains_key("userId") {
            return Err("cookie db 里没有 passToken/userId".into());
        }
        Ok(map)
    })();
    // 库损坏/被锁时也要清掉临时副本，避免 %TEMP% 堆积
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

// 从含 passToken 的字节流里解析出完整 JSON 对象（大括号配对，跳过字符串内部）
fn match_brace(buf: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (j, &c) in buf.iter().enumerate().skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

fn extract_login_payloads(buf: &[u8]) -> Vec<(Value, usize)> {
    let marker = b"passToken";
    let mut payloads = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = buf[from..].windows(marker.len()).position(|w| w == marker) {
        let hit = from + rel;
        from = hit + marker.len();
        // 从命中位置向前找包围它的 '{'（由内向外逐层尝试）
        let mut openers = Vec::new();
        let mut depth = 0i32;
        let mut j = hit;
        while j > 0 && openers.len() < 8 {
            j -= 1;
            let c = buf[j];
            if c == b'}' {
                depth += 1;
            } else if c == b'{' {
                if depth == 0 {
                    openers.push(j);
                } else {
                    depth -= 1;
                }
            }
        }
        for start in openers {
            let Some(end) = match_brace(buf, start) else { continue };
            let Ok(obj) = serde_json::from_slice::<Value>(&buf[start..=end]) else {
                continue;
            };
            if obj.get("passToken").and_then(|t| t.as_str()).is_some_and(|t| !t.is_empty()) {
                payloads.push((obj, start));
                break;
            }
        }
    }
    payloads
}

fn user_id_fallback() -> Option<String> {
    let p = desktop_user_data().join("xiaomi-last-confirmed.json");
    let text = std::fs::read_to_string(p).ok()?;
    let j: Value = serde_json::from_str(&text).ok()?;
    j.get("userId").map(|v| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

// 扫描分区 HTTP 缓存，返回全部可用凭证候选（mtime 降序，按 passToken 去重）。
// 缓存块可能被 Network Service 短时占用：逐文件跳过 + 整轮重试。
fn collect_cache_candidates() -> (Vec<Creds>, bool, bool) {
    let dir = desktop_user_data()
        .join("Partitions")
        .join("xiaomi-account")
        .join("Cache")
        .join("Cache_Data");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return (vec![], false, true);
    };
    let mut names: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    names.sort();

    let mut had_locks = false;
    for round in 0..4 {
        let mut by_token: HashMap<String, Creds> = HashMap::new();
        let mut any_unreadable = false;
        for path in &names {
            let buf = match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => {
                    any_unreadable = true; // 被占用，跳过
                    continue;
                }
            };
            if !buf.windows(9).any(|w| w == b"passToken") {
                continue;
            }
            let hits = extract_login_payloads(&buf);
            if hits.is_empty() {
                continue;
            }
            let mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as f64)
                .unwrap_or(0.0);
            for (obj, _) in hits {
                let fallback = user_id_fallback();
                let Some(user_id) = obj
                    .get("userId")
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .or(fallback)
                else {
                    continue;
                };
                let cand = Creds {
                    pass_token: obj["passToken"].as_str().unwrap_or_default().to_string(),
                    user_id,
                    c_user_id: obj.get("cUserId").and_then(|v| v.as_str()).map(String::from),
                    mtime,
                };
                let dup = by_token.get(&cand.pass_token);
                if dup.is_none() || mtime > dup.unwrap().mtime {
                    by_token.insert(cand.pass_token.clone(), cand);
                }
            }
        }
        had_locks = had_locks || any_unreadable;
        if by_token.is_empty() {
            if !any_unreadable {
                return (vec![], had_locks, false); // 全读到了还命中不了 = 缓存里确实没有
            }
            if round < 3 {
                std::thread::sleep(Duration::from_millis(400));
            }
            continue;
        }
        let mut cands: Vec<Creds> = by_token.into_values().collect();
        cands.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal));
        return (cands, had_locks, false);
    }
    (vec![], had_locks, false)
}

fn read_pass_token() -> Result<Creds, String> {
    let mut last_err: Option<String> = None;
    for db in cookie_db_candidates() {
        match read_cookie_db(&db) {
            Ok(map) => {
                let creds = Creds {
                    pass_token: map["passToken"].clone(),
                    user_id: map["userId"].clone(),
                    c_user_id: map.get("cUserId").cloned(),
                    mtime: 0.0,
                };
                if !rejected_contains(&creds.pass_token) {
                    return Ok(creds);
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    let (cands, had_locks, dir_missing) = collect_cache_candidates();
    let usable: Vec<&Creds> = cands.iter().filter(|c| !rejected_contains(&c.pass_token)).collect();
    if let Some(c) = usable.first() {
        return Ok((*c).clone());
    }
    if !cands.is_empty() {
        // 候选都在，但全被服务端拒过/已过期 —— 该换登录态了
        return Err("登录凭证已失效，请在 MiMo Desktop 重新登录".into());
    }
    let win_hint = if cfg!(target_os = "windows") && had_locks {
        "；若已登录，可能是凭证文件暂时被占用，刷新时会自动重试"
    } else {
        ""
    };
    Err(format!(
        "未找到小米账号 passToken，请先在 MiMo Desktop 登录{}{}{}",
        win_hint,
        if dir_missing { "（未找到 MiMo Desktop 的缓存目录）" } else { "" },
        match &last_err {
            Some(e) => format!("（{}）", e),
            None => String::new(),
        }
    ))
}

pub async fn read_pass_token_async() -> Result<Creds, String> {
    tokio::task::spawn_blocking(read_pass_token)
        .await
        .map_err(|e| e.to_string())?
}

// ---------- HTTP 基础 ----------

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .user_agent(ua())
        .build()
        .expect("build http client")
}

fn merge_set_cookies(cookies: &mut HashMap<String, String>, resp: &reqwest::Response) {
    for val in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        let Ok(s) = val.to_str() else { continue };
        let pair = s.split(';').next().unwrap_or("");
        if let Some(eq) = pair.find('=') {
            let name = pair[..eq].trim().to_string();
            let value = pair[eq + 1..].trim().to_string();
            // 空值 Set-Cookie（如 userId=）只用于清 Cookie，忽略以免覆盖账号态
            if !value.is_empty() {
                cookies.insert(name, value);
            }
        }
    }
}

fn cookie_header(cookies: &HashMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse_json_payload(body: &str) -> Result<Value, String> {
    let tail = body.split("&&&START&&&").last().unwrap_or("");
    serde_json::from_str(tail).map_err(|e| format!("登录响应解析失败: {}", e))
}

// ---------- 平台会话（platform.xiaomimimo.com） ----------

#[derive(Debug)]
pub struct DError {
    pub msg: String,
    pub cred_rejected: bool,
}

impl DError {
    fn new(msg: impl Into<String>) -> Self {
        DError { msg: msg.into(), cred_rejected: false }
    }
    fn rejected(msg: impl Into<String>) -> Self {
        DError { msg: msg.into(), cred_rejected: true }
    }
    pub fn js(&self) -> String {
        self.msg.clone()
    }
}

impl From<String> for DError {
    fn from(s: String) -> Self {
        DError::new(s)
    }
}
impl From<&str> for DError {
    fn from(s: &str) -> Self {
        DError::new(s)
    }
}

pub struct Session {
    client: reqwest::Client,
    cookies: HashMap<String, String>,
    logged_in: bool,
}

impl Session {
    pub fn new() -> Self {
        Session { client: build_client(), cookies: HashMap::new(), logged_in: false }
    }

    async fn request(&mut self, url: &str) -> Result<reqwest::Response, DError> {
        let resp = self
            .client
            .get(url)
            .header(reqwest::header::COOKIE, cookie_header(&self.cookies))
            .send()
            .await
            .map_err(|e| DError::new(format!("请求失败: {}", e)))?;
        merge_set_cookies(&mut self.cookies, &resp);
        Ok(resp)
    }

    async fn login(&mut self) -> Result<(), DError> {
        let mut cred_err: Option<DError> = None;
        for _ in 0..3 {
            let acc = read_pass_token_async().await.map_err(DError::new)?;
            self.cookies.clear();
            self.cookies.insert("passToken".into(), acc.pass_token.clone());
            self.cookies.insert("userId".into(), acc.user_id.clone());
            if let Some(c) = acc.c_user_id {
                self.cookies.insert("cUserId".into(), c);
            }
            match self.login_with_creds().await {
                Ok(()) => return Ok(()),
                Err(e) if e.cred_rejected => {
                    rejected_insert(acc.pass_token);
                    cred_err = Some(e);
                }
                Err(e) => {
                    // 后续候选已全部拉黑时 read_pass_token 会抛笼统错误，保留更具体的首次登录失败原因
                    return Err(cred_err.unwrap_or(e));
                }
            }
        }
        Err(cred_err.unwrap_or_else(|| DError::new("凭证重试次数已用尽")))
    }

    // 回调必须用平台 401 响应里的 loginUrl（自带 sign），自拼会被拒
    async fn login_with_creds(&mut self) -> Result<(), DError> {
        let probe = self.request(&format!("{}/balance", API)).await?;
        let info: Value = probe.json().await.unwrap_or(Value::Null);
        let login_url = match info.get("loginUrl").and_then(|u| u.as_str()) {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => return Err(DError::new(format!("拿不到 loginUrl: {}", info))),
        };
        let url = if login_url.contains("_json") { login_url } else { login_url + "&_json=true" };
        let resp = self.request(&url).await?;
        let body = resp.text().await.map_err(|e| DError::new(e.to_string()))?;
        let payload = parse_json_payload(&body)?;
        let code = payload.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        let loc = payload.get("location").and_then(|l| l.as_str()).filter(|l| !l.is_empty());
        if code != 0 || loc.is_none() {
            let desc = payload
                .get("desc")
                .or_else(|| payload.get("description"))
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            let expired = code == 70016 || desc.contains("验") || desc.contains("过期");
            // 70016/「登录验证失败」多为缓存里 passToken 已过期，提示重登更可操作
            let msg = if expired {
                "登录凭证已失效，请在 MiMo Desktop 重新登录".to_string()
            } else {
                format!("小米登录失败: {}", if desc.is_empty() { code.to_string() } else { desc })
            };
            return Err(DError::rejected(msg));
        }
        // 跟随 location：account -> /sts（下发平台 Cookie）
        let mut loc = Some(loc.unwrap().to_string());
        for _ in 0..5 {
            let Some(u) = loc else { break };
            let r = self.request(&u).await?;
            let status = r.status().as_u16();
            if (300..400).contains(&status) {
                loc = r
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
            } else {
                let _ = r.text().await;
                loc = None;
            }
        }
        self.logged_in = true;
        Ok(())
    }

    async fn ensure_login(&mut self) -> Result<(), DError> {
        if self.logged_in {
            return Ok(());
        }
        self.login().await
    }

    pub async fn get(&mut self, api_path: &str) -> Result<Value, DError> {
        let url = format!("{}/{}", API, api_path);
        for attempt in 0..2 {
            self.ensure_login().await?;
            let resp = self.request(&url).await?;
            let status = resp.status();
            let data: Value = resp.json().await.map_err(|e| DError::new(format!("响应解析失败: {}", e)))?;
            let unauthorized =
                status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                    || data.get("code").and_then(|c| c.as_i64()) == Some(401);
            if unauthorized && attempt == 0 {
                self.logged_in = false;
                continue;
            }
            if data.get("code").and_then(|c| c.as_i64()) != Some(0) {
                return Err(DError::new(format!("API 错误: {}", data)));
            }
            return Ok(match data.get("data") {
                Some(d) if !d.is_null() => d.clone(),
                _ => data,
            });
        }
        Err(DError::new("unreachable"))
    }
}

// ---------- 桌面端订阅（mimo-server sid=mimopc） ----------

fn new_sub_cookies(acc: &Creds) -> HashMap<String, String> {
    let mut c = HashMap::new();
    c.insert("passToken".into(), acc.pass_token.clone());
    c.insert("userId".into(), acc.user_id.clone());
    if let Some(x) = &acc.c_user_id {
        c.insert("cUserId".into(), x.clone());
    }
    c
}

async fn sub_request(
    client: &reqwest::Client,
    cookies: &mut HashMap<String, String>,
    url: &str,
    send_account: bool,
) -> Option<reqwest::Response> {
    let mut req = client.get(url);
    if send_account {
        req = req.header(reqwest::header::COOKIE, cookie_header(cookies));
    }
    let resp = req.send().await.ok()?;
    merge_set_cookies(cookies, &resp);
    Some(resp)
}

// 链路：/api/user/xiaomi/me 302 签名 loginUrl -> serviceLogin(passToken) -> /api/sts 下发 cookie
async fn mimopc_login() -> Option<HashMap<String, String>> {
    let acc = read_pass_token_async().await.ok()?;
    let mut cookies = new_sub_cookies(&acc);
    let client = build_client();

    let probe = sub_request(&client, &mut cookies, &format!("{}/user/xiaomi/me", SUB_BASE), false).await?;
    let probe_ok = probe.status().is_success();
    let login_url = probe
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    // 已登录：接口直接 200 + code=0，不再走 302 loginUrl
    if probe_ok {
        if let Ok(j) = probe.json::<Value>().await {
            if j.get("code").and_then(|c| c.as_i64()) == Some(0) {
                return Some(cookies);
            }
        }
    }
    let login_url = login_url?;

    let r1 = sub_request(&client, &mut cookies, &format!("{}&_json=true", login_url), true).await?;
    let body = r1.text().await.ok()?;
    let payload = parse_json_payload(&body).ok()?;
    if payload.get("code").and_then(|c| c.as_i64()) != Some(0) {
        return None;
    }
    let mut loc = payload.get("location").and_then(|l| l.as_str()).map(String::from);
    for _ in 0..5 {
        let Some(u) = loc else { break };
        let Some(r) = sub_request(&client, &mut cookies, &u, true).await else {
            return None;
        };
        let status = r.status().as_u16();
        if (300..400).contains(&status) {
            loc = r
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(String::from);
        } else {
            let _ = r.text().await;
            loc = None;
        }
    }
    Some(cookies)
}

static SUB_COOKIES: LazyLock<tokio::sync::Mutex<Option<HashMap<String, String>>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

async fn sub_authed(client: &reqwest::Client, path: &str) -> Option<Value> {
    // 共享单例：锁内完成登录（并发第二路等待，不会双登录），clone 出本地副本
    let mut cookies = {
        let mut guard = SUB_COOKIES.lock().await;
        if guard.is_none() {
            *guard = mimopc_login().await;
        }
        guard.clone()?
    };

    let url = format!("{}/{}", SUB_BASE, path);
    let mut resp = sub_request(client, &mut cookies, &url, true).await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        cookies = mimopc_login().await?;
        resp = sub_request(client, &mut cookies, &url, true).await?;
    }
    let data = if resp.status().is_success() {
        let j: Value = resp.json().await.ok()?;
        if j.get("code").and_then(|c| c.as_i64()) == Some(0) {
            j.get("data").cloned()
        } else {
            None
        }
    } else {
        None
    };
    // 本地副本 merge 回共享（保留另一路独有的键），401 重登后的最新 cookie 生效
    let mut guard = SUB_COOKIES.lock().await;
    match guard.as_mut() {
        Some(shared) => {
            for (k, v) in cookies {
                shared.insert(k, v);
            }
        }
        None => *guard = Some(cookies),
    }
    data
}

async fn fetch_subscription() -> Value {
    let client = build_client();
    let (sub, usage) = tokio::join!(
        sub_authed(&client, "user/xiaomi/subscription/self"),
        sub_authed(&client, "user/usage")
    );
    if sub.is_none() && usage.is_none() {
        *SUB_COOKIES.lock().await = None;
        return Value::Null;
    }
    json!({
        "plan": sub.as_ref().and_then(|s| s.get("current")).cloned().unwrap_or(Value::Null),
        "usage": usage.unwrap_or(Value::Null),
    })
}

// ---------- 快照 ----------

static PLATFORM: LazyLock<tokio::sync::Mutex<Session>> =
    LazyLock::new(|| tokio::sync::Mutex::new(Session::new()));

pub struct Snapshot {
    pub balance: Value,
    pub usage: Value,
    pub detail: Value,
    pub subscription: Value,
    pub ts: u64,
}

impl Snapshot {
    pub fn to_json(&self) -> Value {
        json!({
            "balance": self.balance,
            "usage": self.usage,
            "detail": self.detail,
            "subscription": self.subscription,
            "ts": self.ts,
        })
    }
}

pub async fn snapshot() -> Result<Snapshot, String> {
    let platform = async {
        let mut s = PLATFORM.lock().await;
        let balance = s.get("balance").await.map_err(|e| e.js())?;
        let usage = s.get("tokenPlan/usage").await.map_err(|e| e.js())?;
        let detail = s.get("tokenPlan/detail").await.map_err(|e| e.js())?;
        Ok::<_, String>((balance, usage, detail))
    };
    let (plat, subscription) = tokio::join!(platform, fetch_subscription());
    let (balance, usage, detail) = plat?;
    Ok(Snapshot {
        balance,
        usage,
        detail,
        subscription,
        ts: now_ms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_payload_from_cache_bytes() {
        // 模拟 Chromium 缓存块：噪声 + 包含 passToken 的 serviceLogin JSON
        let payload = r#"{"userId":"12345","passToken":"tok-abc","cUserId":"c9"}"#;
        let mut buf = b"\x00\x01prefix noise ".to_vec();
        buf.extend_from_slice(payload.as_bytes());
        buf.extend_from_slice(b" suffix \x00");
        let hits = extract_login_payloads(&buf);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0["passToken"], "tok-abc");
        assert_eq!(hits[0].0["userId"], "12345");
    }

    #[test]
    fn extract_skips_braces_inside_strings() {
        // 字符串里的花括号出现在 marker 之后：向前匹配必须跳过（marker 之前的反向扫描是 JS 原样行为）
        let payload = r#"{"passToken":"t1","a":"} } { end"}"#;
        let hits = extract_login_payloads(payload.as_bytes());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0["passToken"], "t1");
    }

    #[test]
    fn match_brace_nested() {
        let s = br#"{"x":{"y":1},"passToken":"z"}"#;
        let end = match_brace(s, 0).unwrap();
        assert_eq!(end, s.len() - 1);
    }

    #[test]
    fn cookie_header_and_merge() {
        let mut cookies = HashMap::new();
        cookies.insert("a".into(), "1".into());
        assert_eq!(cookie_header(&cookies), "a=1");
    }

    #[test]
    fn json_payload_split() {
        let v = parse_json_payload("&&&START&&&{\"code\":0,\"location\":\"x\"}").unwrap();
        assert_eq!(v["code"], 0);
    }
}
