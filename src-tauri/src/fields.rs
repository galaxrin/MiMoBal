// 字段注册表与纯计算逻辑（从 fields.js 移植）
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Clone)]
pub struct FieldMeta {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub min: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub max: bool,
    pub cat: &'static str,
}

pub static FIELDS: &[FieldMeta] = &[
    FieldMeta { id: "balance.total", label: "账户余额", kind: "money", min: true, max: false, cat: "余额" },
    FieldMeta { id: "balance.cash", label: "现金余额", kind: "money", min: true, max: false, cat: "余额" },
    FieldMeta { id: "balance.gift", label: "礼品余额", kind: "money", min: true, max: false, cat: "余额" },
    FieldMeta { id: "balance.frozen", label: "冻结金额", kind: "money", min: false, max: false, cat: "余额" },
    FieldMeta { id: "plan.name", label: "套餐名", kind: "text", min: false, max: false, cat: "Token Plan" },
    FieldMeta { id: "plan.periodEnd", label: "套餐到期日", kind: "text", min: true, max: false, cat: "Token Plan" },
    FieldMeta { id: "plan.autoRenew", label: "自动续订", kind: "text", min: false, max: false, cat: "Token Plan" },
    FieldMeta { id: "usage.monthPercent", label: "本月用量 %", kind: "percent", min: false, max: true, cat: "Token Plan" },
    FieldMeta { id: "usage.monthTokens", label: "本月已用 / 总量", kind: "text", min: false, max: false, cat: "Token Plan" },
    FieldMeta { id: "usage.planPercent", label: "套餐总量用量 %", kind: "percent", min: false, max: true, cat: "Token Plan" },
    FieldMeta { id: "sub.title", label: "桌面端订阅名", kind: "text", min: false, max: false, cat: "桌面端订阅" },
    FieldMeta { id: "sub.percent", label: "订阅用量 %（桌面端）", kind: "percent", min: false, max: true, cat: "桌面端订阅" },
    FieldMeta { id: "sub.resetDate", label: "订阅重置日（桌面端）", kind: "text", min: false, max: false, cat: "桌面端订阅" },
];

pub fn field_meta(id: &str) -> Option<&'static FieldMeta> {
    FIELDS.iter().find(|m| m.id == id)
}

#[derive(Serialize, Clone)]
pub struct Value {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num: Option<f64>,
}

fn jnum(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.replace(',', "").parse::<f64>().ok(),
        _ => None,
    }
}

fn truthy(v: Option<&serde_json::Value>) -> bool {
    match v {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
        Some(serde_json::Value::String(s)) => !s.is_empty() && s != "0" && s != "false",
        Some(serde_json::Value::Array(a)) => !a.is_empty(),
        _ => true,
    }
}

pub fn fmt_tokens(n: &serde_json::Value) -> String {
    let x = match jnum(n) {
        Some(x) if x.is_finite() => x,
        _ => {
            return match n {
                serde_json::Value::Null => "0".into(),
                serde_json::Value::String(s) if s.is_empty() => "0".into(),
                other => match other {
                    serde_json::Value::String(s) => s.clone(),
                    _ => other.to_string(),
                },
            }
        }
    };
    for (unit, div) in [("B", 1e9), ("M", 1e6), ("K", 1e3)] {
        if x >= div {
            return format!("{:.2}{}", x / div, unit);
        }
    }
    format!("{}", x)
}

fn money_text(prefix: &str, raw: Option<&serde_json::Value>) -> String {
    match raw {
        None | Some(serde_json::Value::Null) => format!("{}?", prefix),
        Some(serde_json::Value::String(s)) if s.is_empty() => format!("{}?", prefix),
        Some(v) => match v {
            serde_json::Value::String(s) => format!("{}{}", prefix, s),
            _ => format!("{}{}", prefix, v),
        },
    }
}

pub fn date_text(v: Option<&serde_json::Value>) -> String {
    let v = match v {
        None | Some(serde_json::Value::Null) => return "—".into(),
        Some(v) => v,
    };
    if let Some(x) = jnum(v) {
        if x == 0.0 {
            return "—".into();
        }
        let ms = if x > 1e12 { x as i64 } else { (x * 1000.0) as i64 };
        return match chrono::DateTime::from_timestamp_millis(ms) {
            Some(d) => d.format("%Y-%m-%d").to_string(),
            None => x.to_string(),
        };
    }
    match v {
        serde_json::Value::String(s) => s.chars().take(10).collect(),
        other => {
            let s = other.to_string();
            s.chars().take(10).collect()
        }
    }
}

fn flag_on(v: Option<&serde_json::Value>) -> bool {
    match v {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_f64() == Some(1.0),
        Some(serde_json::Value::String(s)) => s == "1" || s == "true" || s == "yes",
        _ => false,
    }
}

// 0~1 小数与 0~100 百分数收敛到 0~100
fn pct100(n: Option<f64>) -> Option<f64> {
    let x = n?;
    if !x.is_finite() || x < 0.0 {
        return None;
    }
    Some(if x <= 1.0 { x * 100.0 } else { x })
}

fn usage_pct(item: &serde_json::Value) -> f64 {
    if let Some(p) = pct100(jnum(item.get("percent").unwrap_or(&serde_json::Value::Null))) {
        return p;
    }
    let used = jnum(item.get("used").unwrap_or(&serde_json::Value::Null)).unwrap_or(0.0);
    let limit = jnum(item.get("limit").unwrap_or(&serde_json::Value::Null)).unwrap_or(0.0);
    if limit > 0.0 {
        (100.0 * used / limit).min(100.0)
    } else {
        0.0
    }
}

fn first_item<'a>(v: &'a serde_json::Value, path: &[&str]) -> &'a serde_json::Value {
    static NULL: serde_json::Value = serde_json::Value::Null;
    let mut cur = v;
    for k in path {
        cur = match cur.get(k) {
            Some(x) => x,
            None => return &NULL,
        };
    }
    cur.get("items")
        .and_then(|i| i.get(0))
        .unwrap_or(&NULL)
}

pub fn compute_values(s: &serde_json::Value) -> HashMap<&'static str, Value> {
    let null = serde_json::Value::Null;
    let b = s.get("balance").unwrap_or(&null);
    let d = s.get("detail").unwrap_or(&null);
    let u = s.get("usage").unwrap_or(&null);
    let sub = s.get("subscription").unwrap_or(&null);
    let mi = first_item(u, &["monthUsage"]);
    let pi = first_item(u, &["usage"]);

    let cur = match b.get("currency") {
        Some(serde_json::Value::String(c)) if c != "CNY" => format!("{} ", c),
        _ => "¥".to_string(),
    };
    let month_pct = usage_pct(mi);
    let plan_pct = usage_pct(pi);

    let mut v: HashMap<&'static str, Value> = HashMap::new();
    v.insert(
        "balance.total",
        Value { text: money_text(&cur, b.get("balance")), num: b.get("balance").and_then(jnum) },
    );
    v.insert(
        "balance.cash",
        Value { text: money_text(&cur, b.get("cashBalance")), num: b.get("cashBalance").and_then(jnum) },
    );
    v.insert(
        "balance.gift",
        Value { text: money_text(&cur, b.get("giftBalance")), num: b.get("giftBalance").and_then(jnum) },
    );
    v.insert(
        "balance.frozen",
        Value { text: money_text(&cur, b.get("frozenBalance")), num: b.get("frozenBalance").and_then(jnum) },
    );
    v.insert(
        "plan.name",
        Value {
            text: d.get("planName").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).unwrap_or("—").to_string(),
            num: None,
        },
    );
    v.insert("plan.periodEnd", Value { text: date_text(d.get("currentPeriodEnd")), num: None });
    v.insert(
        "plan.autoRenew",
        Value { text: if flag_on(d.get("enableAutoRenew")) { "开" } else { "关" }.into(), num: None },
    );
    v.insert(
        "usage.monthPercent",
        Value { text: format!("{}%", month_pct.round() as i64), num: Some(month_pct) },
    );
    v.insert(
        "usage.monthTokens",
        Value {
            text: format!("{} / {}", fmt_tokens(mi.get("used").unwrap_or(&null)), fmt_tokens(mi.get("limit").unwrap_or(&null))),
            num: None,
        },
    );
    v.insert(
        "usage.planPercent",
        Value { text: format!("{}%", plan_pct.round() as i64), num: Some(plan_pct) },
    );

    // D7：桌面端订阅（mimo-server sid=mimopc），拿到才有值，否则字段隐藏
    let sub_plan = sub.get("plan").filter(|p| !p.is_null()).unwrap_or(&null);
    let sub_usage = sub.get("usage").filter(|p| !p.is_null()).unwrap_or(&null);
    let sub_title = sub_plan.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty());
    let sub_raw = sub_usage
        .get("percent")
        .filter(|p| !p.is_null())
        .or_else(|| sub_plan.get("percent").filter(|p| !p.is_null()));
    let sub_pct = sub_raw.and_then(|x| pct100(jnum(x)));
    let sub_reset = match sub_usage.get("resetDate") {
        Some(r) if truthy(Some(r)) => Some(display_scalar(r)),
        _ => match sub_plan.get("nextResetTime") {
            Some(p) if !p.is_null() => Some(date_text(Some(p))),
            _ => None,
        },
    };
    if let Some(t) = sub_title {
        v.insert("sub.title", Value { text: t.to_string(), num: None });
    }
    if let Some(p) = sub_pct {
        v.insert("sub.percent", Value { text: format!("{}%", p.round() as i64), num: Some(p) });
    }
    if let Some(r) = sub_reset {
        if r != "—" {
            v.insert("sub.resetDate", Value { text: r, num: None });
        }
    }
    v
}

fn display_scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// 阈值：min/max 可能是 null / 数字 / 字符串（容忍手改 config）
fn thr_num(v: Option<&serde_json::Value>) -> Option<f64> {
    match v {
        None | Some(serde_json::Value::Null) => None,
        Some(x) => jnum(x),
    }
}

pub fn is_red(id: &str, value: Option<&Value>, thresholds: &serde_json::Map<String, serde_json::Value>) -> bool {
    let (min, max) = match thresholds.get(id) {
        Some(serde_json::Value::Object(t)) => (thr_num(t.get("min")), thr_num(t.get("max"))),
        _ => return false,
    };
    let num = match value.and_then(|v| v.num) {
        Some(n) if n.is_finite() => n,
        _ => return false,
    };
    if let Some(m) = min {
        if num < m {
            return true;
        }
    }
    if let Some(m) = max {
        if num > m {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fmt_tokens_scales() {
        assert_eq!(fmt_tokens(&json!(1500000000)), "1.50B");
        assert_eq!(fmt_tokens(&json!(20500)), "20.50K");
        assert_eq!(fmt_tokens(&json!(42)), "42");
        assert_eq!(fmt_tokens(&json!(null)), "0");
    }

    #[test]
    fn pct_convergence() {
        assert_eq!(pct100(Some(0.42)), Some(42.0));
        assert_eq!(pct100(Some(42.0)), Some(42.0));
        assert_eq!(pct100(Some(-1.0)), None);
        assert_eq!(pct100(None), None);
    }

    #[test]
    fn date_text_variants() {
        assert_eq!(date_text(None), "—");
        assert_eq!(date_text(Some(&json!(0))), "—");
        assert_eq!(date_text(Some(&json!(1791423715379i64))), "2026-10-08");
        assert_eq!(date_text(Some(&json!("2026-03-01T00:00:00Z"))), "2026-03-01");
        assert_eq!(date_text(Some(&json!("2026-03-05"))), "2026-03-05");
    }

    #[test]
    fn compute_full_snapshot() {
        let snap = json!({
            "balance": {"balance": "1,234.50", "currency": "CNY"},
            "detail": {"planName": "Pro", "currentPeriodEnd": "2026-12-31", "enableAutoRenew": true},
            "usage": {
                "monthUsage": {"items": [{"used": 500000000, "limit": 1000000000}]},
                "usage": {"items": [{"percent": 0.25}]}
            },
            "subscription": {
                "plan": {"title": "桌面包月", "percent": "50%"},
                "usage": {"percent": 0.8}
            }
        });
        let v = compute_values(&snap);
        assert_eq!(v["balance.total"].text, "¥1,234.50");
        assert_eq!(v["balance.total"].num, Some(1234.5));
        assert_eq!(v["plan.autoRenew"].text, "开");
        assert_eq!(v["usage.monthPercent"].text, "50%");
        assert_eq!(v["usage.monthTokens"].text, "500.00M / 1.00B");
        assert_eq!(v["usage.planPercent"].text, "25%");
        assert_eq!(v["sub.title"].text, "桌面包月");
        assert_eq!(v["sub.percent"].text, "80%");
    }

    #[test]
    fn threshold_red_logic() {
        let values = compute_values(&json!({
            "balance": {"balance": "5.00"},
            "usage": {"monthUsage": {"items": [{"used": 95, "limit": 100}]}, "usage": {"items": [{}]}}
        }));
        let mut th = serde_json::Map::new();
        th.insert("balance.total".into(), json!({"min": 10}));
        th.insert("usage.monthPercent".into(), json!({"max": 90}));
        th.insert("plan.name".into(), json!({"min": 5}));
        assert!(is_red("balance.total", values.get("balance.total"), &th));
        assert!(is_red("usage.monthPercent", values.get("usage.monthPercent"), &th));
        // 文本字段无 num：永不红
        assert!(!is_red("plan.name", values.get("plan.name"), &th));
        assert!(!is_red("balance.cash", values.get("balance.cash"), &th));
    }

    #[test]
    fn flag_and_string_number_tolerance() {
        assert!(flag_on(Some(&json!(true))));
        assert!(flag_on(Some(&json!("1"))));
        assert!(!flag_on(Some(&json!("false"))));
        let mut th = serde_json::Map::new();
        th.insert("balance.total".into(), json!({"min": "10"}));
        let v = Value { text: "¥5".into(), num: Some(5.0) };
        assert!(is_red("balance.total", Some(&v), &th));
    }
}
