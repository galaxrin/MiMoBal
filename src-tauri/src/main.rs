#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod data;
mod fields;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

use serde::Serialize;
use serde_json::Value;
use tauri::menu::{Menu, MenuBuilder, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
#[cfg(target_os = "windows")]
use tauri::tray::TrayIconEvent;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

use config::Config;
use fields::{FieldMeta, FIELDS};

type Shared = Arc<Mutex<Inner>>;

pub struct Inner {
    pub dir: std::path::PathBuf,
    pub config: Config,
    pub snapshot: Option<Value>,
    pub error: Option<String>,
    pub auth_fail_until: u64,
    pub consec_fails: u32,
    pub last_period_end: Option<String>,
    pub prev_red: HashSet<String>,
    pub last_notify_at: HashMap<String, u64>,
    pub quitting: bool,
    pub move_seq: u64,
    pub timer: Option<tauri::async_runtime::JoinHandle<()>>,
}

static REFRESH_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

// ---------- 状态广播 ----------

#[derive(Serialize, Clone)]
struct Entry {
    id: String,
    label: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    num: Option<f64>,
    kind: String,
    red: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PublicState {
    entries: Vec<Entry>,
    tray_entries: Vec<Entry>,
    error: Option<String>,
    // 有旧数据但当前拉取失败：界面灰显（stale）
    stale: bool,
    ts: u64,
    config: Config,
    fields_meta: &'static [FieldMeta],
}

fn ordered_entries(inner: &Inner, target: &str) -> Vec<Entry> {
    let Some(snap) = &inner.snapshot else { return vec![] };
    let values = fields::compute_values(snap);
    let list = if target == "tray" { &inner.config.tray.fields } else { &inner.config.float.fields };
    list.iter()
        .filter(|f| f.on)
        .filter_map(|f| {
            let v = values.get(f.id.as_str())?;
            let meta = fields::field_meta(&f.id)?;
            Some(Entry {
                id: f.id.clone(),
                label: meta.label.to_string(),
                text: v.text.clone(),
                num: v.num,
                kind: meta.kind.to_string(),
                red: fields::is_red(&f.id, Some(v), &inner.config.thresholds),
            })
        })
        .collect()
}

fn public_state(inner: &Inner) -> PublicState {
    PublicState {
        entries: ordered_entries(inner, "float"),
        tray_entries: ordered_entries(inner, "tray"),
        error: inner.error.clone(),
        stale: inner.error.is_some() && inner.snapshot.is_some(),
        ts: inner.snapshot.as_ref().and_then(|s| s.get("ts")).and_then(|t| t.as_u64()).unwrap_or(0),
        config: inner.config.clone(),
        fields_meta: FIELDS,
    }
}

fn broadcast(app: &AppHandle, shared: &Shared) {
    let state = {
        let inner = shared.lock().unwrap();
        public_state(&inner)
    };
    let _ = app.emit("state", state);
    update_tray(app, shared);
}

// ---------- 阈值告警（新变红 → 系统通知，30 分钟限频） ----------

fn check_threshold_alerts(app: &AppHandle, shared: &Shared) {
    let mut inner = shared.lock().unwrap();
    let Some(snap) = &inner.snapshot else { return };
    let values = fields::compute_values(snap);
    let now = data::now_ms();
    let ids: Vec<String> = inner.config.thresholds.keys().cloned().collect();
    let mut notify: Vec<(String, String, String)> = Vec::new();
    for id in ids {
        let v = values.get(id.as_str());
        let red = fields::is_red(&id, v, &inner.config.thresholds);
        if red && !inner.prev_red.contains(&id) {
            let last = inner.last_notify_at.get(&id).copied().unwrap_or(0);
            if now.saturating_sub(last) > 30 * 60 * 1000 {
                inner.last_notify_at.insert(id.clone(), now);
                let label = fields::field_meta(&id).map(|m| m.label).unwrap_or(id.as_str());
                let text = v.map(|x| x.text.clone()).unwrap_or_else(|| "?".into());
                notify.push((id.clone(), label.to_string(), text));
            }
        }
        if red {
            inner.prev_red.insert(id);
        } else {
            inner.prev_red.remove(&id);
        }
    }
    drop(inner);
    for (_id, label, text) in notify {
        println!("[alert] {} -> {}", label, text);
        let _ = app
            .notification()
            .builder()
            .title(format!("MiMoBal · {}触发阈值", label))
            .body(format!("当前 {}，请关注", text))
            .show();
    }
}

// ---------- 周期重置边界（套餐到期日变化 → 通知） ----------

fn check_period_reset(app: &AppHandle, shared: &Shared) {
    let mut inner = shared.lock().unwrap();
    let Some(snap) = &inner.snapshot else { return };
    let Some(new_end) = snap
        .get("detail")
        .and_then(|d| d.get("currentPeriodEnd"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && *s != "—")
        .map(String::from)
    else {
        return;
    };
    match &inner.last_period_end {
        None => inner.last_period_end = Some(new_end), // 首见：只记录不通知
        Some(old) if *old != new_end => {
            inner.last_period_end = Some(new_end.clone());
            drop(inner);
            let _ = app
                .notification()
                .builder()
                .title("MiMoBal · 套餐周期已重置")
                .body(format!("新周期至 {}，用量已刷新", new_end))
                .show();
        }
        _ => {}
    }
}

// ---------- 刷新 ----------

fn auth_looks_failed(msg: &str) -> bool {
    ["凭证已失效", "登录验证失败", "凭证重试次数已用尽", "EXPIRED", "70016"]
        .iter()
        .any(|p| msg.contains(p))
}

async fn refresh(app: AppHandle, shared: Shared, force: bool) {
    let _g = REFRESH_LOCK.lock().await;
    if !force {
        let (until, err) = {
            let inner = shared.lock().unwrap();
            (inner.auth_fail_until, inner.error.clone())
        };
        let now = data::now_ms();
        if now < until && err.as_deref().is_some_and(auth_looks_failed) {
            broadcast(&app, &shared);
            return;
        }
    }
    match data::snapshot().await {
        Ok(snap) => {
            {
                let mut inner = shared.lock().unwrap();
                inner.snapshot = Some(snap.to_json());
                inner.error = None;
                inner.auth_fail_until = 0;
                inner.consec_fails = 0;
            }
            check_threshold_alerts(&app, &shared);
            check_period_reset(&app, &shared);
        }
        Err(e) => {
            let mut inner = shared.lock().unwrap();
            if auth_looks_failed(&e) {
                inner.auth_fail_until = data::now_ms() + 5 * 60 * 1000;
            }
            inner.consec_fails = inner.consec_fails.saturating_add(1);
            inner.error = Some(e);
        }
    }
    broadcast(&app, &shared);
}

// 断网探测：连败退避期间定期探路，网络一恢复立即刷新
async fn probe_online() -> bool {
    use tokio::net::TcpStream;
    use tokio::time::{timeout, Duration};
    timeout(
        Duration::from_secs(3),
        TcpStream::connect(("platform.xiaomimimo.com", 443)),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

fn start_timer(app: AppHandle, shared: Shared) {
    let loop_app = app.clone();
    let loop_shared = shared.clone();
    let handle = tauri::async_runtime::spawn(async move {
        loop {
            // 动态间隔：正常=配置值；连败按 2^n 退避（上限 10 分钟）
            let (base, fails) = {
                let inner = loop_shared.lock().unwrap();
                (inner.config.refresh_sec.max(10), inner.consec_fails)
            };
            let interval = if fails == 0 {
                base
            } else {
                base.saturating_mul(1u64 << fails.min(4)).min(600)
            };
            if fails == 0 {
                tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            } else {
                // 退避窗口内每 15s 探测，通了提前醒；否则到点兜底刷新
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(interval);
                loop {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        break;
                    }
                    let chunk = std::time::Duration::from_secs(15).min(deadline - now);
                    tokio::time::sleep(chunk).await;
                    if probe_online().await {
                        break;
                    }
                }
            }
            refresh(loop_app.clone(), loop_shared.clone(), false).await;
        }
    });
    // 并发调用时 replace 保证只存活最后一个 timer，旧的 abort
    let old = shared.lock().unwrap().timer.replace(handle);
    if let Some(t) = old {
        t.abort();
    }
}

// ---------- 托盘 ----------

fn tray_summary(inner: &Inner) -> String {
    ordered_entries(inner, "tray")
        .iter()
        .map(|e| e.text.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{}…", cut)
}

// 菜单栏空间有限：过长摘要直接截断（与 Electron 版一致，无省略号）
fn truncate_plain(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

// 一次算好托盘摘要/条目，title/tooltip/菜单共用，避免重复 compute_values
struct TrayView {
    summary: String,
    entries: Vec<Entry>,
    float_on: bool,
}

fn tray_view(app: &AppHandle, shared: &Shared) -> TrayView {
    let inner = shared.lock().unwrap();
    TrayView {
        summary: tray_summary(&inner),
        entries: ordered_entries(&inner, "tray"),
        float_on: app.get_webview_window("float").is_some(),
    }
}

fn build_tray_menu(app: &AppHandle, view: &TrayView) -> tauri::Result<Menu<tauri::Wry>> {
    let mut mb = MenuBuilder::new(app);
    if !cfg!(target_os = "macos") && !view.summary.is_empty() {
        for e in &view.entries {
            let item = MenuItem::with_id(
                app,
                format!("entry:{}", e.id),
                format!("{}  {}", e.label, e.text),
                false,
                None::<&str>,
            )?;
            mb = mb.item(&item);
        }
        let sep = PredefinedMenuItem::separator(app)?;
        mb = mb.item(&sep);
    }
    let toggle = MenuItem::with_id(
        app,
        "toggle_float",
        if view.float_on { "隐藏悬浮窗" } else { "显示悬浮窗" },
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    mb.item(&toggle).item(&settings).item(&sep2).item(&quit).build()
}

fn update_tray(app: &AppHandle, shared: &Shared) {
    let state = app.state::<TrayState>();
    let guard = state.0.lock().unwrap();
    let Some(tray) = guard.as_ref() else { return };

    let show_title = shared.lock().unwrap().config.tray.show_title;
    let view = tray_view(app, shared);
    let summary = if show_title { view.summary.clone() } else { String::new() };
    // 有旧数据但拉取失败：tooltip 标注过期（plain title 无法局部着色，菜单栏文字保持原样）
    let stale = {
        let inner = shared.lock().unwrap();
        inner.error.is_some() && inner.snapshot.is_some()
    };

    // 图标垂直微调帧（设置里改 adjust 后随广播即时生效）
    if let Ok(img) = tauri::image::Image::from_bytes(tray_frame_bytes(
        shared.lock().unwrap().config.tray.adjust,
    )) {
        let _ = tray.set_icon(Some(img));
    }

    // 菜单栏文字摘要只在 macOS 有对应物；Windows 托盘没有文字位
    #[cfg(target_os = "macos")]
    {
        let title = if summary.is_empty() {
            String::new()
        } else {
            format!(" {}", truncate_plain(&summary, 72))
        };
        let _ = tray.set_title(Some(title.as_str()));
    }

    let tip = {
        let mut base = if summary.is_empty() {
            "MiMoBal".to_string()
        } else {
            format!("MiMoBal · {}", summary)
        };
        if stale {
            base = format!("⚠ 数据过期 · {}", base);
        }
        if base.chars().count() > 120 {
            truncate_chars(&base, 119)
        } else {
            base
        }
    };
    let _ = tray.set_tooltip(Some(tip.as_str()));

    if let Ok(menu) = build_tray_menu(app, &view) {
        let _ = tray.set_menu(Some(menu));
    }
}

struct TrayState(Mutex<Option<TrayIcon>>);

// 垂直微调帧（-3..+3 pt，正=上移）：不同显示器/DPI 下底对齐兜底；Windows 用彩色图
#[cfg(target_os = "macos")]
static TRAY_FRAMES: [&[u8]; 7] = [
    include_bytes!("../icons/tray_-3.png"),
    include_bytes!("../icons/tray_-2.png"),
    include_bytes!("../icons/tray_-1.png"),
    include_bytes!("../icons/tray_+0.png"),
    include_bytes!("../icons/tray_+1.png"),
    include_bytes!("../icons/tray_+2.png"),
    include_bytes!("../icons/tray_+3.png"),
];

fn tray_frame_bytes(adjust: i32) -> &'static [u8] {
    #[cfg(target_os = "macos")]
    {
        TRAY_FRAMES[(adjust.clamp(-3, 3) + 3) as usize]
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = adjust;
        include_bytes!("../icons/tray-win.png")
    }
}

fn create_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    // mac：M 字形模板图（黑图形+透明底），set_icon_as_template 后随菜单栏明暗自动反色；
    // win：托盘无模板机制，用彩色圆角 logo
    let icon = tauri::image::Image::from_bytes(tray_frame_bytes(0))
        .map_err(|e| format!("tray icon decode: {}", e))?;
    #[allow(unused_mut)]
    let mut builder = TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("MiMoBal")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "quit" => {
                if let Some(shared) = app.try_state::<Shared>() {
                    shared.lock().unwrap().quitting = true;
                }
                app.exit(0);
            }
            "settings" => open_settings(app),
            "toggle_float" => {
                if let Some(shared) = app.try_state::<Shared>() {
                    toggle_float(app, &shared);
                }
            }
            _ => {}
        });
    #[cfg(target_os = "windows")]
    {
        builder = builder.on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, .. } = event {
                open_settings(tray.app_handle());
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    let builder = builder;
    let tray = builder.build(app)?;
    // 模板图只认 alpha：亮色菜单栏显黑、暗色显白
    #[cfg(target_os = "macos")]
    {
        let _ = tray.set_icon_as_template(true);
    }
    *app.state::<TrayState>().0.lock().unwrap() = Some(tray);
    Ok(())
}

// ---------- 悬浮窗 ----------

fn monitor_work_area(app: &AppHandle, center: (f64, f64)) -> Option<(f64, f64, f64, f64)> {
    // 优先包含窗口中心的显示器；退回主显示器。用 work_area（避开菜单栏/任务栏）
    let monitors = app.available_monitors().ok()?;
    let mut picked = None;
    for m in &monitors {
        let a = m.work_area();
        let scale = m.scale_factor();
        let (x, y) = (a.position.x as f64 / scale, a.position.y as f64 / scale);
        let (w, h) = (a.size.width as f64 / scale, a.size.height as f64 / scale);
        if center.0 >= x && center.0 <= x + w && center.1 >= y && center.1 <= y + h {
            picked = Some((x, y, w, h));
            break;
        }
    }
    picked.or_else(|| {
        let m = app.primary_monitor().ok()??;
        let a = m.work_area();
        let scale = m.scale_factor();
        Some((
            a.position.x as f64 / scale,
            a.position.y as f64 / scale,
            a.size.width as f64 / scale,
            a.size.height as f64 / scale,
        ))
    })
}

fn bounds_on_screen(app: &AppHandle, b: &config::Bounds) -> bool {
    if ![b.x, b.y, b.width, b.height].iter().all(|v| v.is_finite()) {
        return false;
    }
    monitor_work_area(app, (b.x + b.width / 2.0, b.y + b.height / 2.0)).is_some()
}

fn float_pos(app: &AppHandle, cfg: &Config) -> (f64, f64) {
    if let Some(b) = &cfg.float.bounds {
        if bounds_on_screen(app, b) {
            return (b.x, b.y);
        }
    }
    let (ax, ay, aw, _ah) =
        monitor_work_area(app, (0.0, 0.0)).unwrap_or((0.0, 0.0, 1920.0, 1080.0));
    (ax + aw - 270.0, ay + 16.0)
}

fn toggle_float(app: &AppHandle, shared: &Shared) {
    if let Some(w) = app.get_webview_window("float") {
        let _ = w.close();
        return;
    }
    let (x, y) = {
        let inner = shared.lock().unwrap();
        float_pos(app, &inner.config)
    };
    let builder = WebviewWindowBuilder::new(app, "float", WebviewUrl::App("float.html".into()))
        .title("MiMoBal")
        .inner_size(120.0, 70.0)
        .min_inner_size(90.0, 56.0)
        .position(x, y)
        .resizable(false)
        // Win11 透明窗角部残影：保持不透明 + DWM 原生圆角。macOS 真透明。
        .transparent(!cfg!(target_os = "windows"))
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .focused(false);
    match builder.build() {
        Ok(w) => {
            // macOS 透明窗必须显式 show 否则不上屏；不 set_focus（悬浮窗不抢焦点）
            let _ = w.show();
            println!(
                "[float] created pos={:?} visible={:?} size={:?} scale={}",
                w.outer_position(),
                w.is_visible(),
                w.inner_size(),
                w.scale_factor().unwrap_or(-1.0)
            );
        }
        Err(e) => {
            eprintln!("[float] create failed: {}", e);
            // 托盘菜单的显隐文案要与实际窗口状态一致
            broadcast(app, shared);
            return;
        }
    }
    // 打开时写入记忆（与关窗对称）；设置胶囊打开时 show 已为 true，此处为兜底
    {
        let mut inner = shared.lock().unwrap();
        if !inner.config.float.show {
            inner.config.float.show = true;
            if let Err(e) = config::save_config(&inner.dir, &inner.config) {
                eprintln!("[config] save failed: {}", e);
            }
        }
    }
    // 统一广播：托盘菜单「显示/隐藏悬浮窗」文案在启动建窗后立即正确
    broadcast(app, shared);
}

fn persist_float_pos(app: AppHandle, shared: Shared, seq: u64) {
    // 拖动停止 400ms 后落盘（与 Electron 版防抖对称）
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        let Some(win) = app.get_webview_window("float") else { return };
        let Ok(pos) = win.outer_position() else { return };
        let scale = win.scale_factor().unwrap_or(1.0);
        let (x, y) = (pos.x as f64 / scale, pos.y as f64 / scale);
        let (w, h) = match win.inner_size() {
            Ok(s) => (s.width as f64 / scale, s.height as f64 / scale),
            Err(_) => return,
        };
        let mut inner = shared.lock().unwrap();
        if inner.move_seq != seq {
            return; // 已有更新的移动事件
        }
        if !bounds_on_screen(&app, &config::Bounds { x, y, width: w, height: h }) {
            return;
        }
        inner.config.float.bounds = Some(config::Bounds { x, y, width: w, height: h });
        if let Err(e) = config::save_config(&inner.dir, &inner.config) {
            eprintln!("[config] save failed: {}", e);
        }
    });
}

fn on_window_event(app: &AppHandle, shared: &Shared, window_label: &str, event: &tauri::WindowEvent) {
    // 设置窗获得焦点 = 用户正在看，立即拉一次新数据
    if window_label == "settings" && matches!(event, tauri::WindowEvent::Focused(true)) {
        let a = app.clone();
        let s = shared.clone();
        tauri::async_runtime::spawn(async move {
            refresh(a, s, false).await;
        });
        return;
    }
    if window_label != "float" {
        return;
    }
    match event {
        tauri::WindowEvent::Moved(_pos) => {
            let seq = {
                let mut inner = shared.lock().unwrap();
                inner.move_seq += 1;
                inner.move_seq
            };
            persist_float_pos(app.clone(), shared.clone(), seq);
        }
        tauri::WindowEvent::Destroyed => {
            let quitting = {
                let mut inner = shared.lock().unwrap();
                // 程序化关闭（设置胶囊关掉悬浮窗等）时同步记忆；退出应用不算
                if !inner.quitting && inner.config.float.show {
                    inner.config.float.show = false;
                    if let Err(e) = config::save_config(&inner.dir, &inner.config) {
                        eprintln!("[config] save failed: {}", e);
                    }
                }
                inner.quitting
            };
            if !quitting {
                broadcast(app, shared); // 内含 update_tray
            }
        }
        _ => {}
    }
}

// ---------- 设置窗 ----------

fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.set_focus();
        return;
    }
    let builder = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("MiMoBal 设置")
        .inner_size(460.0, 620.0);
    if let Err(e) = builder.build() {
        eprintln!("[settings] create failed: {}", e);
    }
}

// ---------- IPC ----------

#[tauri::command]
fn get_state(shared: tauri::State<'_, Shared>) -> PublicState {
    let inner = shared.lock().unwrap();
    public_state(&inner)
}

#[tauri::command]
async fn refresh_now(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<Value, String> {
    // 用户手动重试 = 明确放行登录（例如刚在 MiMo Desktop 重新登录）
    {
        let mut inner = shared.lock().unwrap();
        inner.auth_fail_until = 0;
    }
    let s = shared.inner().clone();
    refresh(app, s, true).await;
    let err = shared.lock().unwrap().error.clone();
    Ok(serde_json::json!({ "error": err }))
}

#[tauri::command]
fn save_config(app: AppHandle, shared: tauri::State<'_, Shared>, next: Value) {
    let (launch_at_login, want_float) = {
        let mut inner = shared.lock().unwrap();
        // bounds 只由主进程在窗口移动时写入：渲染端持有的可能是过期快照
        let bounds = inner.config.float.bounds.clone();
        let mut cfg = config::parse_raw(next);
        cfg.float.bounds = bounds;
        inner.config = cfg;
        if let Err(e) = config::save_config(&inner.dir, &inner.config) {
            eprintln!("[config] save failed: {}", e);
        }
        (inner.config.launch_at_login, inner.config.float.show)
    };

    apply_autostart(&app, launch_at_login);
    start_timer(app.clone(), shared.inner().clone());

    // 悬浮窗开/关与配置对齐（设置页胶囊驱动）
    let has_float = app.get_webview_window("float").is_some();
    if want_float && !has_float {
        toggle_float(&app, &shared.inner().clone());
    } else if !want_float && has_float {
        if let Some(w) = app.get_webview_window("float") {
            let _ = w.close();
        }
    }
    broadcast(&app, &shared.inner().clone());
}

#[tauri::command]
fn set_float_size(app: AppHandle, w: f64, h: f64) {
    let Some(win) = app.get_webview_window("float") else { return };
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return;
    }
    let width = w.ceil().clamp(90.0, 2400.0);
    let height = h.ceil().clamp(56.0, 2000.0);
    let scale = win.scale_factor().unwrap_or(1.0);
    let Ok(cur_size) = win.inner_size() else { return };
    let cur_w = cur_size.width as f64 / scale;
    let cur_h = cur_size.height as f64 / scale;
    if (cur_w - width).abs() <= 1.0 && (cur_h - height).abs() <= 1.0 {
        return;
    }
    let Ok(pos) = win.outer_position() else { return };
    let mut x = pos.x as f64 / scale;
    let y = pos.y as f64 / scale;
    let center = (x + cur_w / 2.0, y + cur_h / 2.0);
    if let Some((wx, _wy, ww, _wh)) = monitor_work_area(&app, center) {
        // 内容变宽时若顶点已贴近屏幕右缘，向左挪，避免增长后滑出工作区
        if x + width > wx + ww {
            x = (wx + ww - width).max(wx);
        }
        if x < wx {
            x = wx;
        }
    }
    // Windows 上 resizable:false 的窗口可能忽略程序化尺寸变更，
    // 先临时放开、改完再收回（视觉无感）
    let _ = win.set_resizable(true);
    let _ = win.set_size(tauri::LogicalSize::new(width, height));
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
    let _ = win.set_resizable(false);
}

#[tauri::command]
fn copy_text(t: String) {
    if t.len() >= 500 {
        return;
    }
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(t);
    }
}

// 开机自启
fn apply_autostart(app: &AppHandle, enable: bool) {
    let mgr = app.autolaunch();
    let res = if enable { mgr.enable() } else { mgr.disable() };
    if let Err(e) = res {
        eprintln!("[login-item] failed: {}", e);
    }
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_settings(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .manage(TrayState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            get_state,
            refresh_now,
            save_config,
            set_float_size,
            copy_text
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            // macOS：菜单栏常驻，隐藏 Dock
            #[cfg(target_os = "macos")]
            let _ = app.set_dock_visibility(false);

            let dir = app
                .path()
                .app_config_dir()
                .map_err(|e| format!("config dir: {}", e))?;
            config::migrate_old_config(&dir);
            let cfg = config::load_config(&dir);
            apply_autostart(&handle, cfg.launch_at_login);

            let shared: Shared = Arc::new(Mutex::new(Inner {
                dir,
                config: cfg,
                snapshot: None,
                error: None,
                auth_fail_until: 0,
                consec_fails: 0,
                last_period_end: None,
                prev_red: HashSet::new(),
                last_notify_at: HashMap::new(),
                quitting: false,
                move_seq: 0,
                timer: None,
            }));
            app.manage(shared.clone());

            create_tray(&handle)?;
            // 立刻生成初始菜单/摘要，不等首轮刷新广播
            update_tray(&handle, &shared);
            println!("[boot] tray ok");

            let rshared = shared.clone();
            let rhandle = handle.clone();
            tauri::async_runtime::spawn(async move {
                refresh(rhandle, rshared, false).await;
            });
            start_timer(handle.clone(), shared.clone());

            if shared.lock().unwrap().config.float.show {
                toggle_float(&handle, &shared);
            }
            println!("[boot] float ok");
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let Some(shared) = window.app_handle().try_state::<Shared>() {
                let label = window.label().to_string();
                on_window_event(window.app_handle(), &shared, &label, event);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building MiMoBal");

    // 任何退出路径（含 Cmd+Q / 系统退出）都置 quitting，
    // 避免窗口销毁时把 float.show 误写成 false。
    // 注意：setup 在 run() 的 Ready 事件里执行，state 只能在 run 回调中取
    app.run(move |handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            if let Some(shared) = handle.try_state::<Shared>() {
                shared.lock().unwrap().quitting = true;
            }
        }
    });
}
