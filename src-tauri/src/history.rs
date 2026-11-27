// 余额/用量采样历史与消耗预测（纯计算，落盘 history.json）
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// 采样节流 30 分钟；只留 7 天；预测只看最近 24 小时
const GAP_MS: u64 = 30 * 60 * 1000;
const KEEP_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const WIN_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct Sample {
    pub t: u64,
    #[serde(default)]
    pub bal: Option<f64>,
    #[serde(default)]
    pub pct: Option<f64>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Trend {
    // 仅供画线：[时间戳, 余额]，按时间升序
    pub points: Vec<[f64; 2]>,
    pub burn_day: Option<f64>,
    pub days_left: Option<f64>,
    pub month_proj: Option<f64>,
    pub days_to_end: Option<i64>,
    // 窗口内余额总体在涨（充值/退款后没有可用斜率，前端给一句说明而不是留空）
    pub rising: bool,
}

fn hist_path(dir: &PathBuf) -> PathBuf {
    dir.join("history.json")
}

pub fn load(dir: &PathBuf, now: u64) -> Vec<Sample> {
    let Ok(text) = std::fs::read_to_string(hist_path(dir)) else {
        return Vec::new();
    };
    let mut v: Vec<Sample> = serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("[history] history.json 解析失败，重新采样: {}", e);
        Vec::new()
    });
    prune(&mut v, now);
    v
}

fn prune(v: &mut Vec<Sample>, now: u64) {
    v.retain(|s| s.t <= now && now - s.t <= KEEP_MS);
}

fn save(dir: &PathBuf, v: &[Sample]) {
    let p = hist_path(dir);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(text) = serde_json::to_string(v) else { return };
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_err() {
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &p) {
        eprintln!("[history] save failed: {}", e);
    }
}

// 采样节流：距上一点不足 GAP 就跳过，避免 10s 刷新把文件写成日志
fn push(v: &mut Vec<Sample>, now: u64, bal: Option<f64>, pct: Option<f64>) -> bool {
    prune(v, now);
    if v.last().is_some_and(|s| now.saturating_sub(s.t) < GAP_MS) {
        return false;
    }
    v.push(Sample { t: now, bal, pct });
    true
}

pub fn record(dir: &PathBuf, v: &mut Vec<Sample>, now: u64, bal: Option<f64>, pct: Option<f64>) {
    if push(v, now, bal, pct) {
        save(dir, v);
    }
}

// 充值（余额上升）和周期重置（用量回落）都会让序列反向跳一次：
// 只用「最后一次反向跳变之后」的段，避免旧基准把斜率算歪
fn tail_after_break(pts: &[(u64, f64)], up_is_break: bool) -> &[(u64, f64)] {
    let mut start = 0;
    for i in 1..pts.len() {
        let d = pts[i].1 - pts[i - 1].1;
        if (up_is_break && d > 0.0) || (!up_is_break && d < 0.0) {
            start = i;
        }
    }
    &pts[start..]
}

// 最小二乘斜率（每天变化量，正=上升）：端点差容易被单点噪声带偏，改用回归
fn slope_per_day(pts: &[(u64, f64)]) -> Option<f64> {
    let first = pts.first()?;
    let span = pts.last()?.0.saturating_sub(first.0) as f64 / 86_400_000.0;
    if span < 0.01 {
        return None; // 跨度不足约 15 分钟：噪声，不给斜率
    }
    let n = pts.len() as f64;
    let xs: Vec<f64> = pts
        .iter()
        .map(|p| p.0.saturating_sub(first.0) as f64 / 86_400_000.0)
        .collect();
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (x, p) in xs.iter().zip(pts) {
        let dx = x - mean_x;
        sxy += dx * (p.1 - mean_y);
        sxx += dx * dx;
    }
    if sxx <= 0.0 {
        return None;
    }
    Some(sxy / sxx)
}

fn days_until(end: &str, now: u64) -> Option<i64> {
    let end = chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d").ok()?;
    let now = chrono::DateTime::from_timestamp_millis(now as i64)?;
    // 取 UTC 日期差：周期末日期只到天，时区抖动会带来 ±1 天误差，不值得为它做本地时区换算
    Some((end - now.date_naive()).num_days())
}

pub fn trend(v: &[Sample], now: u64, period_end: Option<&str>) -> Trend {
    let start = v.partition_point(|s| s.t.saturating_add(WIN_MS) < now);
    let win = &v[start.min(v.len())..];
    let bal: Vec<(u64, f64)> = win.iter().filter_map(|s| s.bal.map(|b| (s.t, b))).collect();
    let pct: Vec<(u64, f64)> = win.iter().filter_map(|s| s.pct.map(|p| (s.t, p))).collect();

    let mut t = Trend {
        points: bal.iter().map(|&(a, b)| [a as f64, b]).collect(),
        ..Default::default()
    };
    t.burn_day = slope_per_day(tail_after_break(&bal, true)).filter(|d| *d < 0.0).map(|d| -d);
    if let (Some(burn), Some(&(_, last))) = (t.burn_day, bal.last()) {
        if last > 0.0 {
            t.days_left = Some(last / burn);
        }
    }
    t.days_to_end = period_end.and_then(|e| days_until(e, now));
    t.rising = matches!((bal.first(), bal.last()), (Some(&(_, f)), Some(&(_, l))) if l > f);
    let rise = slope_per_day(tail_after_break(&pct, false)).filter(|d| *d > 0.0);
    if let (Some(days), Some(r), Some(&(_, last))) = (t.days_to_end, rise, pct.last()) {
        if days > 0 {
            t.month_proj = Some(last + r * days as f64);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u64 = 3_600_000;

    fn s(t: u64, bal: f64, pct: f64) -> Sample {
        Sample { t, bal: Some(bal), pct: Some(pct) }
    }

    #[test]
    fn sample_throttle_and_prune() {
        let mut v = Vec::new();
        let now = 1000 * H;
        assert!(push(&mut v, now, Some(50.0), Some(10.0)));
        assert!(!push(&mut v, now + 20 * 60_000, Some(49.0), Some(11.0))); // 半个小时内跳过
        assert!(push(&mut v, now + GAP_MS, Some(49.0), Some(11.0)));
        assert_eq!(v.len(), 2);
        // 8 天前的点被剪掉
        v.insert(0, s(now - KEEP_MS - H, 60.0, 5.0));
        prune(&mut v, now + GAP_MS);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn balance_drop_projects_days_left() {
        let now = 1000 * H;
        let v = vec![s(now - 24 * H, 100.0, 60.0), s(now, 88.0, 72.0)];
        let t = trend(&v, now, None);
        assert_eq!(t.burn_day, Some(12.0));
        assert_eq!(t.days_left, Some(88.0 / 12.0));
        assert_eq!(t.points.len(), 2);
    }

    #[test]
    fn top_up_or_flat_has_no_burn() {
        let now = 1000 * H;
        let v = vec![s(now - 12 * H, 100.0, 60.0), s(now, 130.0, 62.0), s(now + H, 130.0, 62.0)];
        let t = trend(&v, now + H, None);
        assert_eq!(t.burn_day, None);
        assert_eq!(t.days_left, None);
    }

    #[test]
    fn usage_projects_to_period_end() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-05T12:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let v = vec![s(now - 24 * H, 100.0, 60.0), s(now, 88.0, 72.0)];
        let t = trend(&v, now, Some("2026-01-10"));
        assert_eq!(t.days_to_end, Some(5));
        assert_eq!(t.month_proj, Some(132.0));
    }

    #[test]
    fn old_points_ignored_and_short_window_noise_skipped() {
        let now = 1000 * H;
        let v = vec![s(now - 48 * H, 200.0, 10.0), s(now - 5 * 60_000, 100.0, 60.0), s(now, 99.0, 61.0)];
        let t = trend(&v, now, None);
        assert_eq!(t.points.len(), 2); // 48h 前的点不进窗口
        assert_eq!(t.burn_day, None); // 窗口跨度 5 分钟，太短不给斜率
    }

    #[test]
    fn top_up_restarts_baseline() {
        let now = 1000 * H;
        let v = vec![
            s(now - 6 * H, 100.0, 60.0),
            s(now - 4 * H, 90.0, 62.0),
            s(now - 2 * H, 130.0, 64.0), // 充值：旧的 100→90 基准作废
            s(now, 128.0, 66.0),
        ];
        let t = trend(&v, now, None);
        // 只认充值后的两点：2 小时掉 2 元 → 每天 24 元
        assert_eq!(t.burn_day, Some(24.0));
        assert!((t.days_left.unwrap() - 128.0 / 24.0).abs() < 1e-9);
        assert!(t.rising); // 整体仍在上涨（充值)
    }

    #[test]
    fn slope_damps_endpoint_noise() {
        let now = 1000 * H;
        // 严格递减（不能有回升，回升会被当成充值），但末点是个明显低点
        let v = vec![
            s(now - 24 * H, 100.0, 60.0),
            s(now - 16 * H, 96.0, 62.0),
            s(now - 8 * H, 95.0, 64.0),
            s(now, 80.0, 66.0),
        ];
        let t = trend(&v, now, None);
        // 回归斜率约 -18.3/天；只取首末两点会得出 -20/天
        assert!((t.burn_day.unwrap() - 18.3).abs() < 0.05);
    }

    #[test]
    fn period_reset_restarts_pct_baseline() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-05T12:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let v = vec![
            s(now - 6 * H, 100.0, 60.0),
            s(now - 4 * H, 100.0, 72.0),
            s(now - 2 * H, 100.0, 8.0), // 周期重置：用量回零
            s(now, 100.0, 10.0),
        ];
        let t = trend(&v, now, Some("2026-01-10"));
        assert_eq!(t.burn_day, None); // 余额没动
        // 只看重置后的两点：2 小时涨 2 个点 → 每天 24 点，剩 5 天
        assert!((t.month_proj.unwrap() - (10.0 + 24.0 * 5.0)).abs() < 1e-6);
    }

    #[test]
    fn rising_balance_flagged() {
        let now = 1000 * H;
        let v = vec![
            s(now - 2 * H, 100.0, 60.0),
            s(now - H, 105.0, 61.0),
            s(now, 110.0, 62.0),
        ];
        let t = trend(&v, now, None);
        assert_eq!(t.burn_day, None);
        assert_eq!(t.days_left, None);
        assert!(t.rising);
    }
}
