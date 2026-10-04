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
  for (const [unit, div] of [['B', 1e9], ['M', 1e6], ['K', 1e3]]) {
    if (n >= div) return `${(n / div).toFixed(2)}${unit}`;
  }
  return String(n);
}

function computeValues(s) {
  const b = s.balance || {};
  const d = s.detail || {};
  const u = s.usage || {};
  const mi = (u.monthUsage && u.monthUsage.items && u.monthUsage.items[0]) || {};
  const pi = (u.usage && u.usage.items && u.usage.items[0]) || {};
  const sub = s.subscription;
  const v = {
    'balance.total': { text: `¥${b.balance ?? '?'}`, num: parseFloat(b.balance) },
    'balance.cash': { text: `¥${b.cashBalance ?? '?'}`, num: parseFloat(b.cashBalance) },
    'balance.gift': { text: `¥${b.giftBalance ?? '?'}`, num: parseFloat(b.giftBalance) },
    'balance.frozen': { text: `¥${b.frozenBalance ?? '?'}`, num: parseFloat(b.frozenBalance) },
    'plan.name': { text: d.planName || '—' },
    'plan.periodEnd': { text: (d.currentPeriodEnd || '—').slice(0, 10) },
    'plan.autoRenew': { text: d.enableAutoRenew ? '开' : '关' },
    'usage.monthPercent': { text: `${Math.round((mi.percent || 0) * 100)}%`, num: (mi.percent || 0) * 100 },
    'usage.monthTokens': { text: `${fmtTokens(mi.used || 0)} / ${fmtTokens(mi.limit || 0)}` },
    'usage.planPercent': { text: `${Math.round((pi.percent || 0) * 100)}%`, num: (pi.percent || 0) * 100 },
  };
  // D7：桌面端订阅（mimo-server sid=mimopc），拿到才有值，否则字段隐藏
  const subTitle = sub && sub.plan ? sub.plan.title : null;
  const subPct = sub && sub.usage && sub.usage.percent != null
    ? sub.usage.percent
    : (sub && sub.plan ? sub.plan.percent : null);
  const subReset = sub && sub.usage && sub.usage.resetDate
    ? sub.usage.resetDate
    : (sub && sub.plan ? String(sub.plan.nextResetTime || '').slice(0, 10) : null);
  if (subTitle) {
    v['sub.title'] = { text: subTitle };
  }
  if (subPct != null) {
    v['sub.percent'] = { text: `${Math.round(subPct)}%`, num: subPct };
  }
  if (subReset) {
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
