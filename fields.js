// 字段注册表与纯计算逻辑（无 Electron 依赖，可独立复用/测试）
'use strict';

const FIELDS = [
  { id: 'balance.total', label: '账户余额', kind: 'money', min: true, cat: '余额' },
  { id: 'balance.cash', label: '现金余额', kind: 'money', min: true, cat: '余额' },
  { id: 'balance.gift', label: '礼品余额', kind: 'money', min: true, cat: '余额' },
  { id: 'balance.frozen', label: '冻结金额', kind: 'money', cat: '余额' },
  { id: 'plan.name', label: '套餐名', kind: 'text', cat: 'Token Plan' },
  { id: 'plan.periodEnd', label: '套餐到期日', kind: 'text', min: true, cat: 'Token Plan' },
  { id: 'plan.autoRenew', label: '自动续订', kind: 'text', cat: 'Token Plan' },
  { id: 'usage.monthPercent', label: '本月用量 %', kind: 'percent', max: true, cat: 'Token Plan' },
  { id: 'usage.monthTokens', label: '本月已用 / 总量', kind: 'text', cat: 'Token Plan' },
  { id: 'usage.planPercent', label: '套餐总量用量 %', kind: 'percent', max: true, cat: 'Token Plan' },
  { id: 'sub.title', label: '桌面端订阅名', kind: 'text', cat: '桌面端订阅' },
  { id: 'sub.percent', label: '订阅用量 %（桌面端）', kind: 'percent', max: true, cat: '桌面端订阅' },
  { id: 'sub.resetDate', label: '订阅重置日（桌面端）', kind: 'text', cat: '桌面端订阅' },
];

function fmtTokens(n) {
  const x = Number(n);
  if (!Number.isFinite(x)) return n == null || n === '' ? '0' : String(n);
  for (const [unit, div] of [['B', 1e9], ['M', 1e6], ['K', 1e3]]) {
    if (x >= div) return `${(x / div).toFixed(2)}${unit}`;
  }
  return String(x);
}

// 接口金额可能是 "1,234.50"；解析前去掉千分位
function moneyNum(raw) {
  if (raw == null || raw === '') return NaN;
  return parseFloat(String(raw).replace(/,/g, ''));
}

function moneyText(prefix, raw) {
  return `${prefix}${raw == null || raw === '' ? '?' : raw}`;
}

// 非字符串日期（epoch / Date）直接 .slice 会炸掉整轮刷新
function dateText(v) {
  if (v == null || v === '') return '—';
  if (v instanceof Date) {
    return Number.isNaN(v.getTime()) ? '—' : v.toISOString().slice(0, 10);
  }
  if (typeof v === 'number' && Number.isFinite(v)) {
    if (v === 0) return '—'; // 接口用 0 表示无日期，避免渲染成 1970
    // >1e12 视为毫秒，否则按秒
    const d = new Date(v > 1e12 ? v : v * 1000);
    return Number.isNaN(d.getTime()) ? String(v) : d.toISOString().slice(0, 10);
  }
  return String(v).slice(0, 10);
}

// 字符串 "false"/"0" 不能当 true
function flagOn(v) {
  return v === true || v === 1 || v === '1' || v === 'true' || v === 'yes';
}

// 0~1 小数与 0~100 百分数收敛到 0~100
function pct100(n) {
  const x = Number(n);
  if (!Number.isFinite(x) || x < 0) return null;
  return x <= 1 ? x * 100 : x;
}

function usagePct(item) {
  if (!item) return 0;
  const fromField = pct100(item.percent);
  if (fromField != null) return fromField;
  // 原型 Python 版：percent 缺失时用 used/limit
  const used = Number(item.used) || 0;
  const limit = Number(item.limit) || 0;
  return limit > 0 ? Math.min(100, (used / limit) * 100) : 0;
}

function computeValues(s) {
  const b = s.balance || {};
  const d = s.detail || {};
  const u = s.usage || {};
  const mi = (u.monthUsage && u.monthUsage.items && u.monthUsage.items[0]) || {};
  const pi = (u.usage && u.usage.items && u.usage.items[0]) || {};
  const sub = s.subscription;
  const cur = b.currency && b.currency !== 'CNY' ? `${b.currency} ` : '¥';
  const monthPct = usagePct(mi);
  const planPct = usagePct(pi);
  const v = {
    'balance.total': { text: moneyText(cur, b.balance), num: moneyNum(b.balance) },
    'balance.cash': { text: moneyText(cur, b.cashBalance), num: moneyNum(b.cashBalance) },
    'balance.gift': { text: moneyText(cur, b.giftBalance), num: moneyNum(b.giftBalance) },
    'balance.frozen': { text: moneyText(cur, b.frozenBalance), num: moneyNum(b.frozenBalance) },
    'plan.name': { text: d.planName || '—' },
    'plan.periodEnd': { text: dateText(d.currentPeriodEnd) },
    'plan.autoRenew': { text: flagOn(d.enableAutoRenew) ? '开' : '关' },
    'usage.monthPercent': { text: `${Math.round(monthPct)}%`, num: monthPct },
    'usage.monthTokens': { text: `${fmtTokens(mi.used || 0)} / ${fmtTokens(mi.limit || 0)}` },
    'usage.planPercent': { text: `${Math.round(planPct)}%`, num: planPct },
  };
  // D7：桌面端订阅（mimo-server sid=mimopc），拿到才有值，否则字段隐藏
  const subTitle = sub && sub.plan ? sub.plan.title : null;
  const subRaw = sub && sub.usage && sub.usage.percent != null
    ? sub.usage.percent
    : (sub && sub.plan ? sub.plan.percent : null);
  const subPct = pct100(subRaw);
  const subReset = sub && sub.usage && sub.usage.resetDate
    ? sub.usage.resetDate
    : (sub && sub.plan ? dateText(sub.plan.nextResetTime) : null);
  if (subTitle) {
    v['sub.title'] = { text: subTitle };
  }
  if (subPct != null) {
    v['sub.percent'] = { text: `${Math.round(subPct)}%`, num: subPct };
  }
  if (subReset && subReset !== '—') {
    v['sub.resetDate'] = { text: subReset };
  }
  return v;
}

function isRed(id, value, thresholds) {
  const t = thresholds[id];
  if (!t || value == null || Number.isNaN(value.num)) return false;
  if (t.min != null && t.min !== '' && value.num < Number(t.min)) return true;
  if (t.max != null && t.max !== '' && value.num > Number(t.max)) return true;
  return false;
}

module.exports = { FIELDS, fmtTokens, computeValues, isRed };
