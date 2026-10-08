// 配置加载与保存（从 main.js 移植）
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::fields::FIELDS;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FieldToggle {
    pub id: String,
    #[serde(default)]
    pub on: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FloatCfg {
    #[serde(default = "d_true")]
    pub show: bool,
    #[serde(default = "d_opacity")]
    pub opacity: f64,
    #[serde(default = "d_density")]
    pub density: String,
    #[serde(default = "d_mode")]
    pub mode: String,
    #[serde(default)]
    pub hero_ids: Vec<Option<String>>,
    #[serde(default)]
    pub bounds: Option<Bounds>,
    #[serde(default)]
    pub fields: Vec<FieldToggle>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrayCfg {
    #[serde(default = "d_true")]
    pub show_title: bool,
    // 菜单栏图标垂直微调（pt，-3..3，正=上移），不同显示器/DPI 下对齐兜底
    #[serde(default)]
    pub adjust: i32,
    #[serde(default)]
    pub fields: Vec<FieldToggle>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "d_refresh")]
    pub refresh_sec: u64,
    #[serde(default)]
    pub launch_at_login: bool,
    #[serde(default)]
    pub float: FloatCfg,
    #[serde(default)]
    pub tray: TrayCfg,
    #[serde(default)]
    pub thresholds: Map<String, Value>,
}

fn d_true() -> bool {
    true
}
fn d_opacity() -> f64 {
    0.96
}
fn d_density() -> String {
    "std".into()
}
fn d_mode() -> String {
    "full".into()
}
fn d_refresh() -> u64 {
    60
}

fn default_fields() -> Vec<FieldToggle> {
    ["balance.total", "usage.monthPercent", "plan.name", "plan.periodEnd"]
        .iter()
        .map(|id| FieldToggle { id: id.to_string(), on: true })
        .collect()
}

impl Default for FloatCfg {
    fn default() -> Self {
        FloatCfg {
            show: true,
            opacity: 0.96,
            density: "std".into(),
            mode: "full".into(),
            hero_ids: vec![None, None],
            bounds: None,
            fields: default_fields(),
        }
    }
}

impl Default for TrayCfg {
    fn default() -> Self {
        TrayCfg { show_title: true, adjust: 0, fields: default_fields() }
    }
}

// 保留已知字段顺序，补上缺失字段（off），剔除非法/重复 id
pub fn merge_field_list(list: &[FieldToggle]) -> Vec<FieldToggle> {
    let mut arr = Vec::new();
    let mut known: Vec<&str> = Vec::new();
    for f in list {
        if !known.contains(&f.id.as_str()) && FIELDS.iter().any(|m| m.id == f.id) {
            known.push(&f.id);
            arr.push(FieldToggle { id: f.id.clone(), on: f.on });
        }
    }
    for m in FIELDS {
        if !known.contains(&m.id) {
            known.push(m.id);
            arr.push(FieldToggle { id: m.id.to_string(), on: false });
        }
    }
    arr
}

pub fn config_path(dir: &PathBuf) -> PathBuf {
    dir.join("config.json")
}

pub fn parse_raw(raw: Value) -> Config {
    sanitize(raw)
}

// 手改 config.json 可能写入非法值：收敛，避免秒级轮询风暴等
fn sanitize(raw: Value) -> Config {
    // 迁移：旧版单一 fields 列表 → 悬浮窗/托盘两份
    let legacy: Option<Vec<FieldToggle>> = raw
        .get("fields")
        .and_then(|f| f.as_array())
        .and_then(|arr| serde_json::from_value(Value::Array(arr.clone())).ok());

    let refresh_sec = raw
        .get("refreshSec")
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite())
        .map(|v| v.round() as i64)
        .map(|v| v.clamp(10, 86400) as u64)
        .unwrap_or_else(d_refresh);

    let launch_at_login = raw.get("launchAtLogin").and_then(|v| v.as_bool()).unwrap_or(false);
    let thresholds = raw
        .get("thresholds")
        .and_then(|t| t.as_object())
        .cloned()
        .unwrap_or_default();

    let tray_raw = raw.get("tray").cloned().unwrap_or(Value::Null);
    let float_raw = raw.get("float").cloned().unwrap_or(Value::Null);

    let list_of = |v: &Value, key: &str| -> Option<Vec<FieldToggle>> {
        v.get(key)
            .and_then(|f| f.as_array())
            .and_then(|a| serde_json::from_value::<Vec<FieldToggle>>(Value::Array(a.clone())).ok())
    };

    let tray_defaults = TrayCfg::default();
    let mut cfg = Config::default();
    cfg.refresh_sec = refresh_sec;
    cfg.launch_at_login = launch_at_login;
    cfg.thresholds = thresholds;
    cfg.tray = TrayCfg {
        show_title: tray_raw.get("showTitle").and_then(|v| v.as_bool()).unwrap_or(tray_defaults.show_title),
        adjust: tray_raw
            .get("adjust")
            .and_then(|v| v.as_i64())
            .map(|v| v.clamp(-3, 3) as i32)
            .unwrap_or(0),
        fields: merge_field_list(&list_of(&tray_raw, "fields").or(legacy.clone()).unwrap_or_else(default_fields)),
    };

    let fd = FloatCfg::default();
    let opacity = float_raw
        .get("opacity")
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.3, 1.0))
        .unwrap_or(fd.opacity);
    cfg.float = FloatCfg {
        show: float_raw.get("show").and_then(|v| v.as_bool()).unwrap_or(fd.show),
        opacity,
        density: float_raw
            .get("density")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(fd.density),
        mode: float_raw
            .get("mode")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(fd.mode),
        hero_ids: float_raw
            .get("heroIds")
            .and_then(|h| h.as_array())
            .map(|a| {
                a.iter()
                    .take(2)
                    .map(|x| match x {
                        Value::String(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .filter(|v: &Vec<Option<String>>| !v.is_empty())
            .unwrap_or_else(|| vec![None, None]),
        bounds: float_raw.get("bounds").and_then(|b| serde_json::from_value(b.clone()).ok()),
        fields: merge_field_list(&list_of(&float_raw, "fields").or(legacy).unwrap_or_else(default_fields)),
    };
    cfg
}

pub fn load_config(dir: &PathBuf) -> Config {
    let p = config_path(dir);
    let Ok(text) = std::fs::read_to_string(&p) else {
        return sanitize(Value::Null);
    };
    // 容忍 UTF-8 BOM（外部工具写入），解析失败回退默认而不是静默重置
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
    match serde_json::from_str::<Value>(&text) {
        Ok(raw) => sanitize(raw),
        Err(e) => {
            eprintln!("[config] config.json 解析失败，使用默认配置: {}", e);
            sanitize(Value::Null)
        }
    }
}

pub fn save_config(dir: &PathBuf, cfg: &Config) -> Result<(), String> {
    let p = config_path(dir);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    // 原子写：先写临时文件再 rename，写盘中断不会留下半截 config.json
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

// 改名迁移：userData 目录随产品名变化（mimo-dash / MiMoBal → Tauri 目录），
// 首次启动把旧配置搬过来，避免开关/字段设置丢失
pub fn migrate_old_config(new_dir: &PathBuf) {
    let app_data = new_dir.parent().map(|p| p.to_path_buf());
    let Some(app_data) = app_data else { return };
    let dst = config_path(new_dir);
    if dst.exists() {
        return;
    }
    for old_name in ["MiMoBal", "mimo-dash"] {
        let src = app_data.join(old_name).join("config.json");
        if src.exists() {
            let _ = std::fs::create_dir_all(new_dir);
            match std::fs::copy(&src, &dst) {
                Ok(_) => println!("[migrate] 旧配置已迁移到 {}", dst.display()),
                Err(e) => eprintln!("[migrate] 旧配置迁移失败: {}", e),
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_fills_missing_and_strips_bad() {
        let merged = merge_field_list(&[FieldToggle { id: "plan.name".into(), on: true }, FieldToggle { id: "nope".into(), on: true }]);
        assert_eq!(merged.len(), FIELDS.len());
        assert!(merged.iter().find(|f| f.id == "plan.name").unwrap().on);
        assert!(!merged.iter().find(|f| f.id == "balance.total").unwrap().on);
        assert!(merged.iter().all(|f| f.id != "nope"));
    }

    #[test]
    fn sanitize_clamps_and_migrates_legacy_fields() {
        let raw = json!({
            "refreshSec": 999999,
            "fields": [{"id": "balance.total", "on": false}],
            "float": {"opacity": 5, "heroIds": ["balance.total", "x", "y"]},
            "tray": {"showTitle": false}
        });
        let cfg = sanitize(raw);
        assert_eq!(cfg.refresh_sec, 86400);
        assert!((cfg.float.opacity - 1.0).abs() < 1e-9);
        // JS 语义：slice(0,2) 后仅 null/空串归一为 null，普通字符串保留
        assert_eq!(cfg.float.hero_ids, vec![Some("balance.total".to_string()), Some("x".to_string())]);
        assert!(!cfg.tray.show_title);
        // 旧单列表迁移到两份，balance.total 被显式关掉
        assert!(!cfg.float.fields.iter().find(|f| f.id == "balance.total").unwrap().on);
        assert!(!cfg.tray.fields.iter().find(|f| f.id == "balance.total").unwrap().on);
        assert_eq!(cfg.float.fields.len(), FIELDS.len());
    }

    #[test]
    fn sanitize_bad_refresh_falls_back() {
        let cfg = sanitize(json!({"refreshSec": "soon"}));
        assert_eq!(cfg.refresh_sec, 60);
    }

    #[test]
    fn thresholds_tolerance() {
        let cfg = sanitize(json!({"thresholds": {"balance.total": {"min": "10", "max": null}}}));
        let t = cfg.thresholds.get("balance.total").unwrap();
        assert_eq!(t["min"], json!("10"));
    }
}
