use crate::platform::{self, current_platform_name};
use crate::providers::{
  all_provider_infos, classify_top_level_url, get_provider, is_verified_callback_origin,
  NavigationDecision, ProviderId, ProviderInfo,
};
use crate::storage::{
  detect_conflicts, load_or_migrate_store, normalize_uuid, now_ms, save_store_atomic, AccountKey,
  AccountRecord, AccountsStoreFile, AuthState, ConflictItem, QuotaDetail,
};
use serde::Serialize;
use std::collections::HashMap;
#[cfg(desktop)]
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

pub const APP_TITLE: &str = "叠号";
#[cfg(desktop)]
const LEFT_PANE_WIDTH: f64 = 380.0;
#[cfg(desktop)]
const MAX_WARM_DESKTOP_VIEWS: usize = 6;

#[cfg(desktop)]
const LOGIN_CAPTURE_INIT_JS: &str = r#"(() => {
  try {
    if (location.protocol !== 'https:') return;
    if (window !== window.top) return;
    const initHost = (location.hostname || '').toLowerCase();
    const isPrimaryHost = initHost === 'muse.ai' || initHost.endsWith('.muse.ai') || initHost === 'app.hark.com' || initHost.endsWith('.hark.com');
    const origFetch = window.fetch;
    if (initHost === 'app.hark.com' || initHost.endsWith('.hark.com')) {
      try {
        if (document.documentElement && !document.documentElement.style.backgroundColor) {
          document.documentElement.style.backgroundColor = '#171d2f';
        }
      } catch (_) {}
    }
    const setQuota = (text, pct) => {
      if (!text) return;
      const cleanPct = typeof pct === 'number' && Number.isFinite(pct)
        ? Math.max(0, Math.min(100, Math.round(pct)))
        : null;
      const obj = { text: String(text).slice(0, 48), pct: cleanPct };
      window.__maamQuota = obj;
      try { localStorage.setItem('__maam_cached_quota__', JSON.stringify(obj)); } catch (_) {}
    };
    const saveDetail = (detail) => {
      if (!detail || !detail.planLabel) return;
      window.__maamQuotaDetail = detail;
      try { localStorage.setItem('__maam_cached_quota_detail_v2__', JSON.stringify(detail)); } catch (_) {}
      const rem = typeof detail.planUsedPercent === 'number'
        ? Math.max(0, Math.min(100, 100 - detail.planUsedPercent))
        : null;
      setQuota(detail.planValue || detail.planLabel, rem);
    };
    const formatHarkReset = (iso) => {
      if (!iso || typeof iso !== 'string') return null;
      const d = new Date(iso);
      if (Number.isNaN(d.getTime())) return null;
      const pad = (n) => String(n).padStart(2, '0');
      const hhmm = pad(d.getHours()) + ':' + pad(d.getMinutes());
      const now = new Date();
      const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
      const targetStart = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
      const diffDays = Math.round((targetStart - todayStart) / 86400000);
      if (diffDays === 0) return '今天 ' + hhmm + ' 重置';
      if (diffDays === 1) return '明天 ' + hhmm + ' 重置';
      if (d.getFullYear() === now.getFullYear()) {
        return (d.getMonth() + 1) + '月' + d.getDate() + '日 ' + hhmm + ' 重置';
      }
      return d.getFullYear() + '年' + (d.getMonth() + 1) + '月' + d.getDate() + '日 ' + hhmm + ' 重置';
    };
    const mapHarkSub = (node) => {
      if (!(location.hostname || '').toLowerCase().endsWith('hark.com')) return null;
      if (!node || typeof node !== 'object') return null;
      const meters = (node.meters && typeof node.meters === 'object' ? (node.meters.harkTokens || node.meters) : null)
        || (node.harkTokens && typeof node.harkTokens === 'object' ? node.harkTokens : null)
        || (('dailyLimit' in node || 'poolLimit' in node) ? node : null);
      if (meters && typeof meters === 'object' && (typeof meters.dailyLimit === 'number' || typeof meters.poolLimit === 'number')) {
        const rawPlan = (node.plan && typeof node.plan === 'object' && typeof node.plan.name === 'string' && node.plan.name.trim())
          || (typeof node.planName === 'string' && node.planName.trim())
          || (typeof node.plan === 'string' && node.plan.trim())
          || '';
        const shortPlan = rawPlan.replace(/^hark\s+/i, '').trim();
        const hasDaily = typeof meters.dailyLimit === 'number' && meters.dailyLimit > 0 && typeof meters.dailyUsed === 'number';
        const hasMonthly = typeof meters.poolLimit === 'number' && meters.poolLimit > 0 && typeof meters.poolUsed === 'number';
        if (hasDaily || hasMonthly) {
          const dailyPct = hasDaily ? Math.max(0, Math.min(100, Math.round((meters.dailyUsed / meters.dailyLimit) * 100))) : null;
          const monthlyPct = hasMonthly ? Math.max(0, Math.min(100, Math.round((meters.poolUsed / meters.poolLimit) * 100))) : null;
          if (hasDaily) {
            return {
              planLabel: shortPlan ? (shortPlan + ' · 每日') : '每日上限',
              planValue: '已使用 ' + dailyPct + '%',
              planSubtext: formatHarkReset(meters.dailyResetsAt),
              planUsedPercent: dailyPct,
              extraLabel: hasMonthly ? '每月上限' : null,
              extraValue: hasMonthly ? ('已使用 ' + monthlyPct + '%') : null,
              extraSubtext: hasMonthly ? formatHarkReset(meters.poolResetsAt) : null,
              extraUsedPercent: monthlyPct
            };
          }
          return {
            planLabel: shortPlan ? (shortPlan + ' · 每月') : '每月上限',
            planValue: '已使用 ' + monthlyPct + '%',
            planSubtext: formatHarkReset(meters.poolResetsAt),
            planUsedPercent: monthlyPct,
            extraLabel: null,
            extraValue: null,
            extraSubtext: null,
            extraUsedPercent: null
          };
        }
      }
      const u = node.usage || node.billing || node.credits || node.subscription || node;
      if (!u || typeof u !== 'object' || 'page' in u || 'items' in u || 'messages' in u) return null;
      const rem = u.remaining ?? u.remaining_credits ?? u.remainingCredits ?? u.credits_remaining ?? u.creditsRemaining ?? u.balance ?? u.creditBalance ?? u.left;
      const tot = u.total ?? u.limit ?? u.total_credits ?? u.totalCredits ?? u.max ?? u.allowance;
      const used = u.used ?? u.used_credits ?? u.usedCredits ?? u.consumed;
      const planName = u.plan_name || u.planName || u.plan || node.plan_name || node.planName || node.plan || (typeof node.tier === 'string' ? node.tier : null) || (typeof u.name === 'string' ? u.name : null) || 'Hark 额度';
      if (typeof rem === 'number' || typeof used === 'number') {
        let usedPct = null;
        let planVal = '';
        if (typeof tot === 'number' && tot > 0) {
          const actualUsed = typeof used === 'number' ? used : (tot - (typeof rem === 'number' ? rem : 0));
          usedPct = Math.max(0, Math.min(100, Math.round((actualUsed / tot) * 100)));
          planVal = '已使用 ' + usedPct + '%';
        } else if (typeof rem === 'number') {
          planVal = '剩余 ' + rem;
        }
        const resetsAt = u.resets_at ?? u.resetsAt;
        return {
          planLabel: String(planName),
          planValue: planVal,
          planSubtext: resetsAt ? ('将在 ' + new Date(resetsAt).toLocaleDateString() + ' 重置') : (u.period ? ('周期: ' + u.period) : null),
          planUsedPercent: usedPct,
          extraLabel: null,
          extraValue: null,
          extraSubtext: null,
          extraUsedPercent: null
        };
      }
      return null;
    };
    const mapSub = (node) => {
      if (!node || typeof node !== 'object') return null;
      const harkRes = mapHarkSub(node);
      if (harkRes) return harkRes;
      const s = (node.subscription && typeof node.subscription === 'object' && node.subscription.usage)
        ? node.subscription
        : node;
      const u = s.usage;
      if (!u || typeof u !== 'object') return null;
      if (!('state' in u) && !('percentUsed' in u) && !('percent_used' in u) && !('quotaStatus' in u) && !('quota_status' in u)) {
        return null;
      }
      const subType = s.subscriptionType || s.subscription_type || '';
      const tierName = (s.tier && typeof s.tier.name === 'string' && s.tier.name.trim()) || '';
      const planLabel = (typeof s.usageRowLabel === 'string' && s.usageRowLabel.trim())
        || (typeof s.usage_row_label === 'string' && s.usage_row_label.trim())
        || tierName
        || (subType === 'PAID' ? '订阅版' : subType === 'COMPLIMENTARY' ? '赠送版' : subType === 'FREE' ? '免费版' : '基础额度');
      const rawPct = typeof u.percentUsed === 'number' ? u.percentUsed : u.percent_used;
      const qStatus = u.quotaStatus || u.quota_status || '';
      let planUsedPercent = null;
      if (u.state !== 'UNLIMITED' && typeof rawPct === 'number' && Number.isFinite(rawPct)) {
        planUsedPercent = Math.max(0, Math.min(100, Math.round(rawPct)));
      } else if (qStatus === 'DEPLETED') {
        planUsedPercent = 100;
      }
      const rawVal = (typeof s.usageRowValueLabel === 'string' && s.usageRowValueLabel.trim())
        || (typeof s.usage_row_value_label === 'string' && s.usage_row_value_label.trim())
        || '';
      const planValue = rawVal
        || (u.state === 'UNLIMITED' ? '无限额度'
          : planUsedPercent !== null ? ('已使用 ' + planUsedPercent + '%')
          : qStatus === 'DEPLETED' ? '已用尽'
          : qStatus === 'LOW' ? '额度偏低'
          : '额度充足');
      let planSubtext = (typeof s.statusSubtitle === 'string' && s.statusSubtitle.trim())
        || (typeof s.status_subtitle === 'string' && s.status_subtitle.trim())
        || (typeof s.usageRowFooter === 'string' && s.usageRowFooter.trim())
        || (typeof s.usage_row_footer === 'string' && s.usage_row_footer.trim())
        || null;
      if (!planSubtext) {
        const rawReset = u.resetsAt ?? u.resets_at;
        if (typeof rawReset === 'number' && rawReset > 1000000000) {
          const ms = rawReset > 100000000000 ? rawReset : rawReset * 1000;
          const d = new Date(ms);
          if (!Number.isNaN(d.getTime())) {
            planSubtext = '将在 ' + (d.getMonth() + 1) + '月' + d.getDate() + '日重置';
          }
        }
      }
      const topupVal = (typeof s.topupRowValueLabel === 'string' && s.topupRowValueLabel.trim())
        || (typeof s.topup_row_value_label === 'string' && s.topup_row_value_label.trim())
        || '';
      const topupBal = typeof s.topupBalance === 'number' ? s.topupBalance : s.topup_balance;
      const topupTot = typeof s.topupTotal === 'number' ? s.topupTotal : s.topup_total;
      let extraLabel = null;
      let extraValue = null;
      let extraSubtext = null;
      let extraUsedPercent = null;
      if (topupVal || (typeof topupTot === 'number' && topupTot > 0)) {
        extraLabel = (typeof s.topupRowLabel === 'string' && s.topupRowLabel.trim())
          || (typeof s.topup_row_label === 'string' && s.topup_row_label.trim())
          || '额外的使用额度';
        if (typeof topupTot === 'number' && topupTot > 0 && typeof topupBal === 'number') {
          extraUsedPercent = Math.max(0, Math.min(100, Math.round(((topupTot - topupBal) / topupTot) * 100)));
        }
        extraValue = topupVal || (extraUsedPercent !== null ? ('已使用 ' + extraUsedPercent + '%') : null);
        extraSubtext = '从不过期';
      }
      return {
        planLabel,
        planValue,
        planSubtext,
        planUsedPercent,
        extraLabel,
        extraValue,
        extraSubtext,
        extraUsedPercent
      };
    };
    window.__maamMapSub = mapSub;
    window.__maamSaveDetail = saveDetail;

    const scanTpChunk = (chunk) => {
      if (!Array.isArray(chunk)) return;
      for (let i = 0; i < chunk.length; i++) {
        const item = chunk[i];
        if (Array.isArray(item)) {
          scanTpChunk(item);
          continue;
        }
        if (typeof item === 'function') {
          let src = '';
          try { src = Function.prototype.toString.call(item); } catch (_) { continue; }
          if (src.includes('fetchSubscriptionAction')) {
            const mAct = src.match(/createServerReference\)\("([0-9a-f]{32,64})"[^)]*"fetchSubscriptionAction"\)/);
            if (mAct) window.__maamSubActionId = mAct[1];
            if (i > 0 && typeof chunk[i - 1] === 'number') {
              window.__maamSubModId = chunk[i - 1];
            }
          }
          if (src.includes('preloadHatchSettingsData')) {
            const mPre = src.match(/\.A\((\d+)\)\.then\(\(\{preloadHatchSettingsData\b/);
            if (mPre) window.__maamPreloadModId = parseInt(mPre[1], 10);
          }
        }
      }
    };

    if ((location.hostname || '').endsWith('muse.ai')) {
      try {
        const tpArr = Array.isArray(globalThis.TURBOPACK) ? globalThis.TURBOPACK : [];
        tpArr.forEach(scanTpChunk);
        const origArrPush = Array.prototype.push;
        tpArr.push = function(...items) {
          items.forEach(scanTpChunk);
          return origArrPush.apply(this, items);
        };
        let tpRuntime = (!Array.isArray(globalThis.TURBOPACK) && globalThis.TURBOPACK) ? globalThis.TURBOPACK : null;
        if (tpRuntime && typeof tpRuntime.push === 'function') {
          const origObjPush = tpRuntime.push.bind(tpRuntime);
          tpRuntime.push = function(...items) {
            items.forEach(scanTpChunk);
            return origObjPush(...items);
          };
        } else {
          Object.defineProperty(globalThis, 'TURBOPACK', {
            configurable: true,
            enumerable: true,
            get() {
              return tpRuntime || tpArr;
            },
            set(val) {
              if (val && typeof val === 'object' && !Array.isArray(val) && typeof val.push === 'function') {
                const origPush = val.push.bind(val);
                val.push = function(...items) {
                  items.forEach(scanTpChunk);
                  return origPush(...items);
                };
                tpRuntime = val;
              } else {
                tpRuntime = val;
              }
            }
          });
        }
      } catch (_) {}
    }

    window.__maamRefreshBackendQuota = async (force) => {
      if (window.__maamQuotaInflight) return;
      const now = Date.now();
      if (!force && window.__maamQuotaFetchedAt && now - window.__maamQuotaFetchedAt < 90000) return;
      const curHost = (location.hostname || '').toLowerCase();
      if (curHost === 'app.hark.com' || curHost.endsWith('.hark.com')) {
        window.__maamQuotaInflight = true;
        try {
          const fetchFn = typeof origFetch === 'function' ? origFetch : window.fetch;
          const res = await fetchFn.call(window, '/api/billing/summary', {
            credentials: 'include',
            headers: { Accept: 'application/json' }
          });
          if (res && res.ok) {
            const data = await res.json();
            const mapped = mapHarkSub(data);
            if (mapped) {
              window.__maamQuotaFetchedAt = Date.now();
              saveDetail(mapped);
            }
          }
          if (!window.__maamApiEmail) {
            try {
              const profRes = await fetchFn.call(window, '/api/profile', {
                credentials: 'include',
                headers: { Accept: 'application/json' }
              });
              if (profRes && profRes.ok) {
                const prof = await profRes.json();
                if (prof && typeof prof.email === 'string' && prof.email.includes('@')) {
                  window.__maamApiEmail = prof.email.trim().toLowerCase();
                }
              }
            } catch (_) {}
          }
        } catch (_) {
        } finally {
          window.__maamQuotaInflight = false;
        }
        return;
      }
      const tpGlobal = globalThis.TURBOPACK;
      if (!tpGlobal || Array.isArray(tpGlobal) || typeof tpGlobal.push !== 'function') return;
      window.__maamQuotaInflight = true;
      try {
        if (!window.__maamTp) {
          tpGlobal.push(['__maam_init__', 999999991, (ctx) => { window.__maamTp = ctx; }]);
          tpGlobal.push(['__maam_run__', { otherChunks: [], runtimeModuleIds: [999999991] }]);
        }
        const tp = window.__maamTp;
        if (!tp) return;
        if (!window.__maamSubModId && !window.__maamPreloadModId && !window.__maamScriptsScanned) {
          window.__maamScriptsScanned = true;
          const scripts = Array.from(document.querySelectorAll('script[src*="/_next/static/"]'));
          for (const sc of scripts) {
            if (!sc.src) continue;
            try {
              const txt = await origFetch.call(window, sc.src, { cache: 'force-cache' }).then((r) => r.text());
              const mPre = txt.match(/\.A\((\d+)\)\.then\(\(\{preloadHatchSettingsData\b/);
              if (mPre) window.__maamPreloadModId = parseInt(mPre[1], 10);
              const mAct = txt.match(/createServerReference\)\("([0-9a-f]{32,64})"[^)]*"fetchSubscriptionAction"\)/);
              if (mAct) window.__maamSubActionId = mAct[1];
              if (window.__maamPreloadModId && window.__maamSubActionId) break;
            } catch (_) {}
          }
        }
        if (!window.__maamSubModId && window.__maamPreloadModId && typeof tp.A === 'function') {
          await tp.A(window.__maamPreloadModId).catch(() => {});
        }
        if (window.__maamSubModId && typeof tp.i === 'function') {
          const mod = tp.i(window.__maamSubModId);
          const fn = mod && (mod.f || mod.default || Object.values(mod).find((x) => typeof x === 'function'));
          if (typeof fn === 'function') {
            const res = await fn({ includeAgreement: true });
            const mapped = mapSub(res);
            if (mapped) {
              window.__maamQuotaFetchedAt = Date.now();
              saveDetail(mapped);
              return;
            }
          }
        }
        let callServerFn = null;
        if (tp.c && typeof tp.c.values === 'function') {
          for (const m of tp.c.values()) {
            if (m && m.exports && typeof m.exports.callServer === 'function') {
              callServerFn = m.exports.callServer;
              break;
            }
          }
        }
        const aid = window.__maamSubActionId || '40e0249eeae158685b6a8d0c3b203f53dd4f2c7d63';
        if (callServerFn && aid) {
          const res = await callServerFn(aid, [{ includeAgreement: true }]);
          const mapped = mapSub(res);
          if (mapped) {
            window.__maamQuotaFetchedAt = Date.now();
            saveDetail(mapped);
          }
        }
      } catch (_) {
      } finally {
        window.__maamQuotaInflight = false;
      }
    };

    const inspectNode = (node, depth, skipSub) => {
      if (!node || typeof node !== 'object' || depth > 6) return;
      if (Array.isArray(node)) {
        for (let i = 0; i < node.length && i < 15; i++) inspectNode(node[i], depth + 1, skipSub);
        return;
      }
      if (typeof node.email === 'string' && /^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$/.test(node.email.trim())) {
        window.__maamApiEmail = node.email.trim().toLowerCase();
      }
      if (typeof node.avatar_url === 'string' && node.avatar_url.startsWith('http')) {
        window.__maamApiAvatar = node.avatar_url;
      }
      if (typeof node.picture === 'string' && node.picture.startsWith('http')) {
        window.__maamApiAvatar = node.picture;
      }
      let nextSkipSub = Boolean(skipSub);
      if (!nextSkipSub) {
        const mapped = mapSub(node);
        if (mapped) {
          saveDetail(mapped);
          nextSkipSub = true;
        }
      }
      const remCredits = node.remaining_credits ?? node.credits_remaining ?? node.credit_balance ?? node.quota_remaining;
      if (typeof remCredits === 'number' && Number.isFinite(remCredits)) {
        const total = node.total_credits ?? node.monthly_limit ?? node.quota_limit;
        if (typeof total === 'number' && total > 0) {
          const pct = Math.max(0, Math.min(100, Math.round((remCredits / total) * 100)));
          setQuota('剩余 ' + remCredits + '/' + total, pct);
        } else {
          setQuota('剩余额度 ' + remCredits, null);
        }
      }
      if (node.code === 'out_of_credits' || node.error === 'rate_limit_exceeded') {
        setQuota('额度已用尽', 0);
      }
      const keys = Object.keys(node);
      for (let i = 0; i < keys.length && i < 24; i++) {
        const v = node[keys[i]];
        if (v && typeof v === 'object') inspectNode(v, depth + 1, nextSkipSub);
      }
    };
    const inspectText = (txt) => {
      if (typeof txt !== 'string' || txt.length < 2 || txt.length > 600000) return;
      if (!/percent_used|percentUsed|quota_status|quotaStatus|usage_row_value_label|usageRowValueLabel|topup_balance|topupBalance|remaining_credits|credit_balance|out_of_credits|UNLIMITED|email|credits|allowance|balance|usage|tokens|billing|subscription|harkTokens|dailyLimit|dailyUsed|poolLimit|poolUsed|dailyResetsAt|poolResetsAt|meters/i.test(txt)) return;
      try {
        inspectNode(JSON.parse(txt), 0, false);
        return;
      } catch (_) {}
      const lines = txt.split('\n');
      for (let i = 0; i < lines.length && i < 80; i++) {
        const idx = lines[i].indexOf(':');
        if (idx > 0 && idx < 12) {
          const rest = lines[i].slice(idx + 1).trim();
          if (rest.startsWith('{') || rest.startsWith('[')) {
            try { inspectNode(JSON.parse(rest), 0, false); } catch (_) {}
          }
        }
      }
    };
    if (isPrimaryHost) {
      if (typeof origFetch === 'function') {
        window.fetch = function(...args) {
          const reqUrl = typeof args[0] === 'string' ? args[0] : (args[0] && args[0].url) || '';
          const p = origFetch.apply(this, args);
          p.then((res) => {
            try {
              const ct = (res.headers && res.headers.get('content-type')) || '';
              if (ct.includes('json') || ct.includes('text')) {
                res.clone().text().then(inspectText).catch(() => {});
              }
              if (reqUrl.includes('/api/chat') || reqUrl.includes('/api/messages')) {
                clearTimeout(window.__maamPostChatTimer);
                window.__maamPostChatTimer = setTimeout(() => {
                  if (typeof window.__maamRefreshBackendQuota === 'function') {
                    window.__maamRefreshBackendQuota(true);
                  }
                }, 8000);
              }
            } catch (_) {}
          }).catch(() => {});
          return p;
        };
      }
      const OrigWS = window.WebSocket;
      if (typeof OrigWS === 'function') {
        const WrappedWS = function(...args) {
          const ws = new OrigWS(...args);
          ws.addEventListener('message', (ev) => {
            if (typeof ev.data === 'string') inspectText(ev.data);
          });
          return ws;
        };
        WrappedWS.prototype = OrigWS.prototype;
        Object.assign(WrappedWS, OrigWS);
        window.WebSocket = WrappedWS;
      }
    }
    const capture = (el) => {
      if (!el || typeof el.value !== 'string') return;
      const v = el.value.trim();
      if (/^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$/.test(v)) {
        window.__maamTypedIdentity = v.toLowerCase();
        try { sessionStorage.setItem('__maam_typed_identity__', window.__maamTypedIdentity); } catch (_) {}
      } else if (/^\+?\d[\d\s-]{6,16}$/.test(v) && (el.type === 'tel' || /手机|phone|tel/i.test(el.placeholder || el.name || ''))) {
        window.__maamTypedPhone = v.replace(/\s+/g, '');
        try { sessionStorage.setItem('__maam_typed_phone__', window.__maamTypedPhone); } catch (_) {}
      }
    };
    document.addEventListener('input', (e) => capture(e.target), true);
    document.addEventListener('change', (e) => capture(e.target), true);
    document.addEventListener('blur', (e) => capture(e.target), true);
    document.addEventListener('submit', () => {
      document.querySelectorAll('input').forEach(capture);
    }, true);
    window.addEventListener('keydown', (e) => {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey) {
        const m = e.key && e.key.match(/^[1-9]$/);
        if (m) {
          window.__maamPendingHotkey = parseInt(m[0], 10);
        }
      }
    }, true);
  } catch (_) {}
})();"#;

#[cfg(desktop)]
const DOM_PAINT_READY_JS: &str = r#"(() => {
  try {
    if (location.href === 'about:blank') return false;
    if (document.readyState === 'loading') return false;
    const body = document.body;
    if (!body) return false;
    const host = (location.hostname || '').toLowerCase();
    const isHark = host === 'app.hark.com' || host.endsWith('.hark.com');
    const isVisibleEl = (el) => {
      if (!el) return false;
      const r = el.getBoundingClientRect();
      if (r.width < 16 || r.height < 16) return false;
      try {
        const st = getComputedStyle(el);
        if (st.display === 'none' || st.visibility === 'hidden' || st.opacity === '0') return false;
      } catch (_) {}
      return true;
    };
    if (isHark) {
      if (document.readyState !== 'complete') return false;
      const sheets = document.querySelectorAll('link[rel~="stylesheet"]');
      for (const l of sheets) {
        if (l.sheet === null) return false;
      }
      const root = document.getElementById('root');
      const rootText = root ? (root.innerText || '').trim() : '';
      if (rootText.length < 4 && (body.innerText || '').trim() === 'Unauthorized') {
        return false;
      }
      if (root) {
        let rootReady = rootText.length >= 2;
        if (!rootReady) {
          const uiEls = root.querySelectorAll('input, button, textarea, [contenteditable="true"], svg, img');
          for (let i = 0; i < uiEls.length && i < 30; i++) {
            if (isVisibleEl(uiEls[i])) {
              rootReady = true;
              break;
            }
          }
        }
        if (!rootReady) return false;
      }
      if (window.__harkSky && !window.__harkSky.painted) {
        const gl = document.getElementById('gl');
        if (gl) {
          try {
            if (getComputedStyle(gl).visibility === 'hidden') return false;
          } catch (_) {}
        }
      }
      return true;
    }
    const textLen = (body.innerText || '').trim().length;
    if (textLen >= 4) return true;
    const els = body.querySelectorAll('input, button, svg, img, canvas, textarea, [role="button"]');
    for (let i = 0; i < els.length && i < 40; i++) {
      if (isVisibleEl(els[i])) return true;
    }
    return false;
  } catch (_) {
    return false;
  }
})()"#;

#[cfg(desktop)]
const IDENTITY_PROBE_JS: &str = r#"(() => {
  try {
    const pendingHotkey = window.__maamPendingHotkey || null;
    window.__maamPendingHotkey = null;
    document.querySelectorAll('input').forEach((el) => {
      const v = (el.value || '').trim();
      if (/^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$/.test(v)) {
        window.__maamTypedIdentity = v.toLowerCase();
        try { sessionStorage.setItem('__maam_typed_identity__', window.__maamTypedIdentity); } catch (_) {}
      }
    });
    let typed = window.__maamTypedIdentity || '';
    let phone = window.__maamTypedPhone || '';
    try {
      typed = typed || sessionStorage.getItem('__maam_typed_identity__') || '';
      phone = phone || sessionStorage.getItem('__maam_typed_phone__') || '';
    } catch (_) {}
    const host = (location.hostname || '').toLowerCase();
    const path = (location.pathname || '').toLowerCase();
    const isPrimary = host === 'muse.ai' || host.endsWith('.muse.ai') || host === 'app.hark.com' || host.endsWith('.hark.com');
    const isExternalLoginHost = host === 'meta.com' || host.endsWith('.meta.com') || host === 'facebook.com' || host.endsWith('.facebook.com') || host === 'appleid.apple.com';
    const makeLoggedOut = () => {
      window.__maamQuota = null;
      window.__maamQuotaDetail = null;
      try {
        localStorage.removeItem('__maam_cached_quota__');
        localStorage.removeItem('__maam_cached_quota_detail_v2__');
      } catch (_) {}
      return JSON.stringify({ auth: false, loggedOut: true, typed: typed || phone, email: '', avatar: '', quotaText: '', quotaPct: null, quotaDetail: null, pendingHotkey });
    };
    const makeLoading = () => JSON.stringify({ auth: false, loggedOut: false, typed: typed || phone, email: '', avatar: '', quotaText: '', quotaPct: null, quotaDetail: null, pendingHotkey });
    if (document.readyState !== 'complete') {
      return makeLoading();
    }
    const loginInputs = document.querySelectorAll('input[type="email"], input[type="password"], input[autocomplete="one-time-code"], input[placeholder*="邮箱"], input[placeholder*="手机"], input[placeholder*="email" i], input[placeholder*="phone" i]');
    const hasVisibleLoginInput = () => {
      for (const inp of loginInputs) {
        if (inp.disabled || inp.readOnly) continue;
        const r = inp.getBoundingClientRect();
        if (r.width > 20 && r.height > 15) return true;
      }
      return false;
    };
    const hasChatComposer = Array.from(document.querySelectorAll('textarea, [contenteditable="true"]')).some((el) => {
      if (el.disabled || el.readOnly) return false;
      const r = el.getBoundingClientRect();
      return r.width > 120 && r.height > 20;
    });
    if (isExternalLoginHost) {
      if (/^\/(login|signin|signup|auth)\b/.test(path) || hasVisibleLoginInput()) {
        return makeLoggedOut();
      }
      return makeLoading();
    }
    if (!isPrimary) {
      return makeLoading();
    }
    if (/^\/(login|signin|signup|auth)\b/.test(path)) {
      return makeLoggedOut();
    }
    if (!hasChatComposer && !/^\/(settings|profile|account)\b/.test(path) && hasVisibleLoginInput()) {
      return makeLoggedOut();
    }
    const bodyText = (document.body && document.body.innerText) || '';
    if (!hasChatComposer && /登录或创建账户|Log in or sign up|Sign in to|登录以继续|通过 Google 继续|Continue with Google|通过 Apple 继续|Continue with Apple/i.test(bodyText)) {
      return makeLoggedOut();
    }

    const extractAvatar = () => {
      const selectors = [
        '[data-hatch-media-owner="avatar-layer"]',
        '[data-hatch-avatar-host]:not([data-hatch-avatar-host-hidden="true"]) img',
        '[data-hatch-avatar-host]:not([data-hatch-avatar-host-hidden="true"]) video',
        '[data-hatch-avatar-host] img',
        '[data-hatch-avatar-host] video',
        '[data-hatch-avatar-media-static] img',
        '[data-hatch-avatar-media-static] video',
        '[class*="hatch-avatar"] img',
        '[class*="hatch-avatar"] video',
        '[data-avatar-parity-side] img',
        '[data-testid*="avatar" i] img',
        '[data-testid*="user" i] img',
        'header [class*="avatar" i] img',
        'nav [class*="avatar" i] img',
        'button[aria-label*="Profile" i] img',
        'button[aria-label*="Account" i] img',
        'button[aria-label*="User" i] img',
        'button[aria-label*="Settings" i] img',
        'img[src^="blob:"]',
        '[class*="avatar" i] img',
        'img[alt*="avatar" i]',
        'img[src*="avatar" i]'
      ];
      let target = null;
      for (const sel of selectors) {
        const isHatchAvatar = sel.includes('hatch');
        const list = document.querySelectorAll(sel);
        for (const el of list) {
          if (el.closest && el.closest('.landing-rebus-avatar-slot')) continue;
          const r = el.getBoundingClientRect();
          if (!isHatchAvatar && (r.width < 16 || r.height < 16 || r.width > 520 || Math.abs(r.width - r.height) > 64)) continue;
          if (isHatchAvatar && r.width > 520) continue;
          if (el.tagName === 'IMG' && el.complete && el.naturalWidth > 0) {
            target = el;
            break;
          }
          if (el.tagName === 'VIDEO' && el.readyState >= 2 && el.videoWidth > 0) {
            target = el;
            break;
          }
        }
        if (target) break;
      }
      if (!target) return window.__maamApiAvatar || window.__maamAvatarData || '';
      const srcKey = target.currentSrc || target.src || target.poster || '';
      if (srcKey && window.__maamAvatarKey === srcKey && window.__maamAvatarData) {
        return window.__maamAvatarData;
      }
      try {
        const sw = target.tagName === 'VIDEO' ? target.videoWidth : target.naturalWidth;
        const sh = target.tagName === 'VIDEO' ? target.videoHeight : target.naturalHeight;
        if (!sw || !sh) return window.__maamApiAvatar || window.__maamAvatarData || '';
        const size = 72;
        const canvas = document.createElement('canvas');
        canvas.width = size;
        canvas.height = size;
        const ctx = canvas.getContext('2d');
        if (!ctx) return '';
        const side = Math.min(sw, sh);
        const sx = (sw - side) / 2;
        const sy = (sh - side) / 2;
        ctx.drawImage(target, sx, sy, side, side, 0, 0, size, size);
        const px = ctx.getImageData(0, 0, size, size).data;
        let visible = 0;
        for (let i = 3; i < px.length; i += 64) {
          if (px[i] > 12) { visible++; if (visible >= 4) break; }
        }
        if (visible < 4) return window.__maamApiAvatar || window.__maamAvatarData || '';
        let dataUrl = canvas.toDataURL('image/webp', 0.86);
        if (!dataUrl || dataUrl.length < 32) {
          dataUrl = canvas.toDataURL('image/png');
        }
        if (dataUrl && dataUrl.startsWith('data:image/') && dataUrl.length < 60000) {
          window.__maamAvatarKey = srcKey;
          window.__maamAvatarData = dataUrl;
          return dataUrl;
        }
      } catch (_) {}
      return window.__maamApiAvatar || window.__maamAvatarData || '';
    };

    const extractQuota = () => {
      if (isPrimary) {
        if (typeof window.__maamRefreshBackendQuota === 'function') {
          window.__maamRefreshBackendQuota(false);
        }
      }

      if (window.__maamQuotaDetail && window.__maamQuotaDetail.planLabel) {
        const d = window.__maamQuotaDetail;
        const rem = typeof d.planUsedPercent === 'number' ? Math.max(0, 100 - d.planUsedPercent) : null;
        return { text: d.planValue || d.planLabel, pct: rem, detail: d };
      }

      try {
        const cachedV2 = localStorage.getItem('__maam_cached_quota_detail_v2__');
        if (cachedV2) {
          const d = JSON.parse(cachedV2);
          if (d && d.planLabel) {
            window.__maamQuotaDetail = d;
            const rem = typeof d.planUsedPercent === 'number' ? Math.max(0, 100 - d.planUsedPercent) : null;
            return { text: d.planValue || d.planLabel, pct: rem, detail: d };
          }
        }
      } catch (_) {}

      if (host.endsWith('hark.com')) {
        const mDaily = bodyText.match(/(?:已使用每日上限的\s*(\d+(?:\.\d+)?)\s*%|(\d+(?:\.\d+)?)\s*%\s*of\s*daily\s*limit\s*used)/i);
        const mMonthly = bodyText.match(/(?:已使用每月上限的\s*(\d+(?:\.\d+)?)\s*%|(\d+(?:\.\d+)?)\s*%\s*of\s*monthly\s*limit\s*used)/i);
        if (mDaily || mMonthly) {
          const dPct = mDaily ? Math.max(0, Math.min(100, Math.round(parseFloat(mDaily[1] || mDaily[2])))) : null;
          const mPct = mMonthly ? Math.max(0, Math.min(100, Math.round(parseFloat(mMonthly[1] || mMonthly[2])))) : null;
          const mDailyReset = bodyText.match(/(?:每日重置|Daily reset)[\s:：·]*([^\n\r]{2,28})/i);
          const mMonthlyReset = bodyText.match(/(?:每月重置|Monthly reset)[\s:：·]*([^\n\r]{2,28})/i);
          const fmtDomReset = (m) => {
            if (!m || !m[1]) return null;
            const s = m[1].trim();
            return s ? (s.endsWith('重置') ? s : (s + ' 重置')) : null;
          };
          const d = dPct !== null
            ? {
                planLabel: '每日上限',
                planValue: '已使用 ' + dPct + '%',
                planSubtext: fmtDomReset(mDailyReset),
                planUsedPercent: dPct,
                extraLabel: mPct !== null ? '每月上限' : null,
                extraValue: mPct !== null ? ('已使用 ' + mPct + '%') : null,
                extraSubtext: mPct !== null ? fmtDomReset(mMonthlyReset) : null,
                extraUsedPercent: mPct
              }
            : {
                planLabel: '每月上限',
                planValue: '已使用 ' + mPct + '%',
                planSubtext: fmtDomReset(mMonthlyReset),
                planUsedPercent: mPct,
                extraLabel: null,
                extraValue: null,
                extraSubtext: null,
                extraUsedPercent: null
              };
          if (typeof window.__maamSaveDetail === 'function') window.__maamSaveDetail(d);
          return { text: d.planValue, pct: 100 - d.planUsedPercent, detail: d };
        }
        const creditEls = document.querySelectorAll('[class*="credit" i], [class*="usage" i], [data-testid*="credit"], [data-testid*="usage"]');
        for (const el of creditEls) {
          const txt = (el.innerText || '').trim();
          const m = txt.match(/(\d+(?:\.\d+)?)\s*\/\s*(\d+(?:\.\d+)?)/);
          if (m) {
            const cur = parseFloat(m[1]);
            const max = parseFloat(m[2]);
            if (max > 0) {
              const pct = Math.max(0, Math.min(100, Math.round((cur / max) * 100)));
              const d = {
                planLabel: 'Hark 用量',
                planValue: '已使用 ' + pct + '%',
                planSubtext: '当前 ' + cur + ' / ' + max,
                planUsedPercent: pct,
                extraLabel: null,
                extraValue: null,
                extraSubtext: null,
                extraUsedPercent: null
              };
              if (typeof window.__maamSaveDetail === 'function') window.__maamSaveDetail(d);
              return { text: d.planValue, pct: 100 - pct, detail: d };
            }
          }
        }
      }

      if (window.__maamQuota && window.__maamQuota.text && window.__maamQuota.text !== '剩余额度 100%') {
        return { text: window.__maamQuota.text, pct: window.__maamQuota.pct, detail: null };
      }
      return { text: '正在同步额度…', pct: null, detail: null };
    };

    const avatar = extractAvatar();
    const quota = extractQuota();

    const bad = /^(support|privacy|help|legal|press|noreply|no-reply|info|security|contact|abuse|feedback|sales|terms)@/i;
    const re = /[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}/g;
    const candidates = [];
    if (window.__maamApiEmail) candidates.push(window.__maamApiEmail);
    document.querySelectorAll('[data-email],[aria-label*="@"],[title*="@"],a[href^="mailto:"]').forEach((el) => {
      const raw = (el.getAttribute('data-email') || el.getAttribute('aria-label') || el.getAttribute('title') || el.getAttribute('href') || '').replace(/^mailto:/i, '');
      const m = raw.match(re);
      if (m) candidates.push(...m);
    });
    try {
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i);
        const val = k ? localStorage.getItem(k) : '';
        if (val && val.includes('@') && val.length < 4000) {
          const m = val.match(re);
          if (m) candidates.push(...m);
        }
      }
    } catch (_) {}
    const textMatches = bodyText.match(re);
    if (textMatches) candidates.push(...textMatches);
    let foundEmail = '';
    for (const c of candidates) {
      const clean = c.toLowerCase();
      if (!bad.test(clean) && !clean.endsWith('@example.com') && !clean.endsWith('@sentry.io')) {
        foundEmail = clean;
        break;
      }
    }
    return JSON.stringify({
      auth: true,
      loggedOut: false,
      typed: typed || phone,
      email: foundEmail,
      avatar,
      quotaText: quota.text || '',
      quotaPct: quota.pct,
      quotaDetail: quota.detail || null,
      pendingHotkey
    });
  } catch (_) {
    return JSON.stringify({ auth: false, loggedOut: false, typed: '', email: '', avatar: '', quotaText: '', quotaPct: null, quotaDetail: null, pendingHotkey: null });
  }
})()"#;

/// Credentials remain in their isolated WebView profile. This export is created only
/// when explicitly requested by the local manager and is never persisted by the app.
#[cfg(desktop)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCookieExport {
  pub provider_id: ProviderId,
  pub uuid: String,
  pub cookie_count: usize,
  pub cookie_header: String,
}

#[cfg(desktop)]
fn format_cookie_header(cookies: &[tauri::webview::Cookie<'static>]) -> String {
  cookies
    .iter()
    .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
    .collect::<Vec<_>>()
    .join("; ")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerSnapshot {
  pub platform: &'static str,
  pub providers: Vec<ProviderInfo>,
  pub accounts: Vec<AccountRecord>,
  pub selected_provider_id: ProviderId,
  pub active_account: Option<AccountKey>,
  pub switching_account: Option<AccountKey>,
  pub warm_accounts: Vec<AccountKey>,
  pub last_committed: Option<AccountKey>,
  pub pending_cleanup: Vec<AccountKey>,
  pub conflicts: Vec<ConflictItem>,
  pub notice: Option<String>,
  pub modal_active: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct PendingExternalAuth {
  account_key: AccountKey,
  switch_seq: u64,
  nonce: String,
}

#[cfg(desktop)]
struct WarmDesktopEntry {
  account_key: AccountKey,
  view_instance_id: u64,
  webview_label: String,
  popup_label: Option<String>,
  paint_ready: bool,
  last_url: Option<Url>,
  captured_identity_hint: Option<String>,
  watcher_running: bool,
  last_used_ms: u64,
  alive_flag: Arc<AtomicBool>,
}

struct ActiveSessionSlot {
  account_key: AccountKey,
  view_instance_id: u64,
  #[cfg_attr(desktop, allow(dead_code))]
  switch_seq: u64,
  committed: bool,
}

struct InnerState {
  store_path: PathBuf,
  app_data_dir: PathBuf,
  store: AccountsStoreFile,
  conflicts: Vec<ConflictItem>,
  selected_provider_id: ProviderId,
  active_slot: Option<ActiveSessionSlot>,
  last_active_per_provider: HashMap<ProviderId, AccountKey>,
  #[cfg(desktop)]
  warm_pool: HashMap<AccountKey, WarmDesktopEntry>,
  #[cfg(desktop)]
  prewarm_failed: HashSet<AccountKey>,
  pending_external_auth: Option<PendingExternalAuth>,
  notice: Option<String>,
  modal_active: bool,
}

pub struct SessionManager {
  current_seq: Arc<AtomicU64>,
  active_view_id: Arc<AtomicU64>,
  view_counter: AtomicU64,
  last_sync_timestamp: AtomicU64,
  inner: Mutex<InnerState>,
}

impl SessionManager {
  pub fn new(app_data_dir: PathBuf) -> Self {
    let store_path = app_data_dir.join("accounts.json");
    let loaded = load_or_migrate_store(&store_path);
    let initial_provider = loaded
      .store
      .last_committed
      .as_ref()
      .map(|k| k.provider_id)
      .filter(|pid| get_provider(*pid).info.enabled)
      .unwrap_or(ProviderId::Muse);

    Self {
      current_seq: Arc::new(AtomicU64::new(1)),
      active_view_id: Arc::new(AtomicU64::new(0)),
      view_counter: AtomicU64::new(1),
      last_sync_timestamp: AtomicU64::new(0),
      inner: Mutex::new(InnerState {
        store_path,
        app_data_dir,
        store: loaded.store,
        conflicts: loaded.conflicts,
        selected_provider_id: initial_provider,
        active_slot: None,
        last_active_per_provider: HashMap::new(),
        #[cfg(desktop)]
        warm_pool: HashMap::new(),
        #[cfg(desktop)]
        prewarm_failed: HashSet::new(),
        pending_external_auth: None,
        notice: loaded.startup_notice,
        modal_active: false,
      }),
    }
  }

  fn snapshot_from_locked(locked: &InnerState) -> ManagerSnapshot {
    let (active_account, switching_account) = match &locked.active_slot {
      Some(slot) if slot.committed => (Some(slot.account_key.clone()), None),
      Some(slot) => (None, Some(slot.account_key.clone())),
      None => (None, None),
    };

    #[cfg(desktop)]
    let warm_accounts = locked
      .warm_pool
      .values()
      .filter(|e| e.paint_ready)
      .map(|e| e.account_key.clone())
      .collect::<Vec<_>>();

    #[cfg(not(desktop))]
    let warm_accounts = Vec::new();

    ManagerSnapshot {
      platform: current_platform_name(),
      providers: all_provider_infos(),
      accounts: locked.store.accounts.clone(),
      selected_provider_id: locked.selected_provider_id,
      active_account,
      switching_account,
      warm_accounts,
      last_committed: locked.store.last_committed.clone(),
      pending_cleanup: locked.store.pending_cleanup.clone(),
      conflicts: locked.conflicts.clone(),
      notice: locked.notice.clone(),
      modal_active: locked.modal_active,
    }
  }

  fn emit_snapshot(&self, app: &AppHandle, snap: &ManagerSnapshot) {
    let _ = app.emit("manager-state-changed", snap);
  }

  pub async fn get_snapshot(&self) -> ManagerSnapshot {
    let locked = self.inner.lock().await;
    Self::snapshot_from_locked(&locked)
  }

  /// Export cookies belonging to the currently active account's WebView only.
  /// Do not call from a synchronous Tauri command on Windows (WebView2 deadlock).
  #[cfg(desktop)]
  pub async fn export_account_cookies(
    &self,
    app: &AppHandle,
    raw_provider: &str,
    raw_uuid: &str,
  ) -> Result<SessionCookieExport, String> {
    let provider_id = ProviderId::parse(raw_provider)
      .ok_or_else(|| "未知服务".to_string())?;
    let uuid = normalize_uuid(raw_uuid).ok_or_else(|| "无效账号 UUID".to_string())?;
    let key = AccountKey { provider_id, uuid: uuid.clone() };
    let label = {
      let locked = self.inner.lock().await;
      let active = locked.active_slot.as_ref()
        .is_some_and(|slot| slot.committed && slot.account_key == key);
      if !active {
        return Err("只能导出当前正在使用的账号，请先切换到该账号".to_string());
      }
      locked.warm_pool.get(&key)
        .map(|entry| entry.webview_label.clone())
        .ok_or_else(|| "账号浏览器尚未就绪".to_string())?
    };
    let webview = app.get_webview(&label)
      .ok_or_else(|| "当前账号浏览器不可用".to_string())?;
    let origin = Url::parse(get_provider(provider_id).info.entry_url)
      .map_err(|_| "服务入口地址无效".to_string())?;
    let cookies = webview.cookies_for_url(origin)
      .map_err(|err| format!("读取当前账号 Cookie 失败: {err}"))?;
    let cookie_count = cookies.len();
    Ok(SessionCookieExport {
      provider_id,
      uuid,
      cookie_count,
      cookie_header: format_cookie_header(&cookies),
    })
  }

  pub async fn run_startup_tasks(self: &Arc<Self>, app: &AppHandle) {
    #[cfg(target_os = "android")]
    {
      let mgr = Arc::clone(self);
      let app_clone = app.clone();
      let channel = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(raw_json) = body {
          if let Ok(evt) =
            serde_json::from_str::<crate::platform::android::AndroidSessionEvent>(&raw_json)
          {
            let mgr_inner = Arc::clone(&mgr);
            let app_inner = app_clone.clone();
            tauri::async_runtime::spawn(async move {
              mgr_inner.handle_android_event(&app_inner, evt).await;
            });
          }
        }
        Ok(())
      });
      let _ = crate::platform::android::subscribe_events(app, channel).await;
    }

    self.retry_pending_cleanups(app).await;

    let mgr_periodic = Arc::clone(self);
    let app_periodic = app.clone();
    tauri::async_runtime::spawn(async move {
      let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5 * 60));
      interval.tick().await;
      loop {
        interval.tick().await;
        mgr_periodic.trigger_silent_sync(&app_periodic, false).await;
      }
    });

    let candidate = {
      let mut locked = self.inner.lock().await;
      if !locked.conflicts.is_empty() || locked.notice.is_some() {
        None
      } else if let Some(key) = locked.store.last_committed.clone() {
        let in_cleanup = locked.store.pending_cleanup.contains(&key);
        let exists = locked
          .store
          .accounts
          .iter()
          .any(|a| a.provider_id == key.provider_id && a.uuid == key.uuid);
        let enabled = get_provider(key.provider_id).info.enabled;
        if in_cleanup || !exists {
          locked.store.last_committed = None;
          let _ = save_store_atomic(&locked.store_path, &locked.store);
          locked.notice = Some("上次记录的账号键不存在或已清理，已回到管理页".to_string());
          None
        } else if !enabled {
          None
        } else {
          locked.selected_provider_id = key.provider_id;
          Some(key)
        }
      } else {
        None
      }
    };

    if let Some(key) = candidate {
      let _ = self.switch_account_internal(app, key, true).await;
    } else {
      let snap = self.get_snapshot().await;
      self.emit_snapshot(app, &snap);
    }

    let mgr_prewarm = Arc::clone(self);
    let app_prewarm = app.clone();
    tauri::async_runtime::spawn(async move {
      tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
      mgr_prewarm.prewarm_next_cold_account(&app_prewarm).await;
    });
  }

  async fn retry_pending_cleanups(&self, app: &AppHandle) {
    let keys = {
      let locked = self.inner.lock().await;
      locked.store.pending_cleanup.clone()
    };
    if keys.is_empty() {
      return;
    }

    for key in keys {
      let deleted = self.delete_profile_for_key(app, &key.uuid).await;
      if deleted.is_ok() {
        let mut locked = self.inner.lock().await;
        locked.store.pending_cleanup.retain(|k| k != &key);
        let _ = save_store_atomic(&locked.store_path, &locked.store);
      }
    }
  }

  async fn delete_profile_for_key(&self, app: &AppHandle, uuid_str: &str) -> Result<(), String> {
    #[cfg(desktop)]
    {
      let app_data_dir = {
        let locked = self.inner.lock().await;
        locked.app_data_dir.clone()
      };
      platform::delete_desktop_profile(app, &app_data_dir, uuid_str).await
    }
    #[cfg(target_os = "android")]
    {
      crate::platform::android::delete_profile(app, uuid_str).await
    }
    #[cfg(all(not(desktop), not(target_os = "android")))]
    {
      let _ = (app, uuid_str);
      Ok(())
    }
  }

  #[cfg(desktop)]
  fn close_warm_entry(app: &AppHandle, entry: WarmDesktopEntry) {
    entry.alive_flag.store(false, Ordering::SeqCst);
    if let Some(popup_label) = entry.popup_label {
      if let Some(popup_win) = app.get_webview_window(&popup_label) {
        let _ = popup_win.close();
      }
    }
    if let Some(wv) = app.get_webview(&entry.webview_label) {
      let _ = wv.hide();
      let _ = wv.close();
    }
  }

  #[cfg(desktop)]
  fn evict_warm_pool_if_needed(
    app: &AppHandle,
    locked: &mut InnerState,
    protect: Option<&AccountKey>,
    max_len: usize,
  ) {
    while locked.warm_pool.len() > max_len {
      let victim_key = locked
        .warm_pool
        .iter()
        .filter(|(k, _)| {
          Some(*k) != protect && locked.active_slot.as_ref().map(|s| &s.account_key) != Some(*k)
        })
        .min_by_key(|(_, entry)| (entry.last_used_ms, entry.view_instance_id))
        .map(|(k, _)| k.clone());
      let Some(victim) = victim_key else {
        break;
      };
      if let Some(entry) = locked.warm_pool.remove(&victim) {
        Self::close_warm_entry(app, entry);
      }
    }
  }

  async fn hide_foreground_slot(&self, app: &AppHandle, next_seq: u64, locked: &mut InnerState) {
    locked.pending_external_auth = None;
    self.active_view_id.store(0, Ordering::SeqCst);
    if let Some(old_slot) = locked.active_slot.take() {
      #[cfg(desktop)]
      {
        let _ = next_seq;
        if let Some(entry) = locked.warm_pool.get_mut(&old_slot.account_key) {
          if let Some(popup_label) = entry.popup_label.take() {
            if let Some(popup_win) = app.get_webview_window(&popup_label) {
              let _ = popup_win.close();
            }
          }
          if let Some(wv) = app.get_webview(&entry.webview_label) {
            let _ = wv.hide();
          }
        }
      }
      #[cfg(target_os = "android")]
      {
        let _ = old_slot;
        let _ = crate::platform::android::close_active_session(app, next_seq).await;
      }
    }
    #[cfg(desktop)]
    {
      if let Some(main_win) = app.get_window("main") {
        let _ = main_win.set_title(APP_TITLE);
      }
      Self::apply_desktop_layout_locked(app, locked);
    }
  }

  #[cfg(desktop)]
  fn set_right_webview_input_frozen(webview: &tauri::Webview, frozen: bool) {
    #[cfg(windows)]
    {
      let _ = webview.with_webview(move |pwv| unsafe {
        let mut hwnd = windows::Win32::Foundation::HWND::default();
        if pwv.controller().ParentWindow(&mut hwnd).is_ok() && !hwnd.0.is_null() {
          let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(hwnd, !frozen);
        }
      });
    }

    if frozen {
      let _ = webview.eval(
        r#"(() => {
          try {
            document.documentElement.inert = true;
            document.activeElement?.blur?.();
            if (!document.getElementById('__mgr_freeze_shield__')) {
              const s = document.createElement('div');
              s.id = '__mgr_freeze_shield__';
              s.style.cssText = 'position:fixed;inset:0;z-index:2147483647;background:transparent;cursor:not-allowed;';
              for (const ev of ['mousedown','mouseup','click','keydown','keyup','keypress','focusin','wheel','touchstart']) {
                s.addEventListener(ev, (e) => { e.stopImmediatePropagation(); e.preventDefault(); }, true);
              }
              document.documentElement.appendChild(s);
            }
          } catch (_) {}
        })()"#,
      );
    } else {
      let _ = webview.eval(
        r#"(() => {
          try {
            document.documentElement.inert = false;
            document.getElementById('__mgr_freeze_shield__')?.remove();
          } catch (_) {}
        })()"#,
      );
    }
  }

  #[cfg(desktop)]
  fn apply_desktop_layout_locked(app: &AppHandle, locked: &InnerState) {
    let Some(main_win) = app.get_window("main") else {
      return;
    };
    let Ok(phys_size) = main_win.inner_size() else {
      return;
    };
    let Ok(scale) = main_win.scale_factor() else {
      return;
    };
    let win_w = (phys_size.width as f64) / scale;
    let win_h = (phys_size.height as f64) / scale;

    let active_committed_label = locked.active_slot.as_ref().and_then(|slot| {
      if slot.committed {
        locked
          .warm_pool
          .get(&slot.account_key)
          .map(|e| e.webview_label.clone())
      } else {
        None
      }
    });

    for entry in locked.warm_pool.values() {
      if Some(&entry.webview_label) != active_committed_label.as_ref() {
        if let Some(wv) = app.get_webview(&entry.webview_label) {
          let _ = wv.hide();
        }
      }
    }

    if let Some(active_label) = active_committed_label {
      let left_w = win_w.min(LEFT_PANE_WIDTH);
      let right_w = (win_w - left_w).max(0.0);

      if let Some(mgr_wv) = app.get_webview("manager") {
        let _ = mgr_wv.set_bounds(tauri::Rect {
          position: tauri::LogicalPosition::new(0.0, 0.0).into(),
          size: tauri::LogicalSize::new(left_w, win_h).into(),
        });
      }

      if let Some(acc_wv) = app.get_webview(&active_label) {
        if right_w <= 1.0 {
          let _ = acc_wv.hide();
        } else {
          let _ = acc_wv.set_bounds(tauri::Rect {
            position: tauri::LogicalPosition::new(left_w, 0.0).into(),
            size: tauri::LogicalSize::new(right_w, win_h).into(),
          });
          let _ = acc_wv.show();
        }
        Self::set_right_webview_input_frozen(&acc_wv, locked.modal_active);
        if locked.modal_active {
          if let Some(mgr_wv) = app.get_webview("manager") {
            let _ = mgr_wv.set_focus();
          }
        }
      }
    } else if let Some(mgr_wv) = app.get_webview("manager") {
      let _ = mgr_wv.set_bounds(tauri::Rect {
        position: tauri::LogicalPosition::new(0.0, 0.0).into(),
        size: tauri::LogicalSize::new(win_w, win_h).into(),
      });
    }
  }

  #[cfg(desktop)]
  pub async fn recompute_desktop_bounds(&self, app: &AppHandle) {
    let locked = self.inner.lock().await;
    Self::apply_desktop_layout_locked(app, &locked);
  }

  pub async fn select_provider(
    self: &Arc<Self>,
    app: &AppHandle,
    raw_provider: &str,
  ) -> Result<ManagerSnapshot, String> {
    let provider_id = ProviderId::parse(raw_provider).ok_or_else(|| "无效的服务标识".to_string())?;
    let next_seq = self.current_seq.fetch_add(1, Ordering::SeqCst) + 1;

    #[cfg(desktop)]
    let warm_restore = {
      let mut locked = self.inner.lock().await;
      locked.selected_provider_id = provider_id;
      self.hide_foreground_slot(app, next_seq, &mut locked).await;
      if get_provider(provider_id).info.enabled && locked.conflicts.is_empty() {
        locked
          .last_active_per_provider
          .get(&provider_id)
          .cloned()
          .filter(|k| {
            locked
              .warm_pool
              .get(k)
              .map(|e| e.paint_ready)
              .unwrap_or(false)
              && locked
                .store
                .accounts
                .iter()
                .any(|a| a.provider_id == k.provider_id && a.uuid == k.uuid)
          })
      } else {
        None
      }
    };

    #[cfg(desktop)]
    if let Some(restore_key) = warm_restore {
      return self.switch_account_internal(app, restore_key, false).await;
    }

    #[cfg(not(desktop))]
    {
      let mut locked = self.inner.lock().await;
      locked.selected_provider_id = provider_id;
      self.hide_foreground_slot(app, next_seq, &mut locked).await;
      return Ok(Self::snapshot_from_locked(&locked));
    }

    #[cfg(desktop)]
    Ok(self.get_snapshot().await)
  }

  pub async fn add_account(
    self: &Arc<Self>,
    app: &AppHandle,
    raw_provider: &str,
    raw_alias: &str,
  ) -> Result<ManagerSnapshot, String> {
    let provider_id = ProviderId::parse(raw_provider).ok_or_else(|| "无效的服务标识".to_string())?;
    if !get_provider(provider_id).info.enabled {
      return Err(format!(
        "{} 当前待接入，暂不支持添加内嵌账号",
        get_provider(provider_id).info.name
      ));
    }
    let trimmed = raw_alias.trim();
    let (alias, auto_alias) = if trimmed.is_empty() {
      ("待登录账号".to_string(), true)
    } else {
      (trimmed.to_string(), false)
    };

    let created_key = {
      let mut locked = self.inner.lock().await;
      if !locked.conflicts.is_empty() {
        return Err("请先处理 UUID 归属冲突后再新增账号".to_string());
      }

      let uuid = loop {
        let candidate = Uuid::new_v4().hyphenated().to_string();
        let used_in_accounts = locked.store.accounts.iter().any(|a| a.uuid == candidate);
        let used_in_cleanup = locked.store.pending_cleanup.iter().any(|k| k.uuid == candidate);
        if !used_in_accounts && !used_in_cleanup {
          break candidate;
        }
      };

      locked.store.accounts.push(AccountRecord {
        provider_id,
        uuid: uuid.clone(),
        alias,
        created_at_ms: now_ms(),
        auto_alias,
        avatar_url: None,
        quota_text: None,
        quota_percent: None,
        quota_detail: None,
        auth_state: AuthState::LoggedOut,
      });
      save_store_atomic(&locked.store_path, &locked.store)?;
      locked.selected_provider_id = provider_id;
      AccountKey { provider_id, uuid }
    };

    self.switch_account_internal(app, created_key, false).await
  }

  pub async fn rename_account(
    &self,
    app: &AppHandle,
    raw_provider: &str,
    raw_uuid: &str,
    raw_alias: &str,
  ) -> Result<ManagerSnapshot, String> {
    let provider_id = ProviderId::parse(raw_provider).ok_or_else(|| "无效的服务标识".to_string())?;
    let uuid = normalize_uuid(raw_uuid).ok_or_else(|| "无效的账号 UUID".to_string())?;
    let alias = raw_alias.trim();
    if alias.is_empty() {
      return Err("账号别名不能为空".to_string());
    }

    let mut locked = self.inner.lock().await;
    let target = locked
      .store
      .accounts
      .iter_mut()
      .find(|a| a.provider_id == provider_id && a.uuid == uuid)
      .ok_or_else(|| "未找到匹配的账号记录，拒绝跨服务改名".to_string())?;
    target.alias = alias.to_string();
    target.auto_alias = false;
    save_store_atomic(&locked.store_path, &locked.store)?;

    if let Some(slot) = &locked.active_slot {
      if slot.committed && slot.account_key.provider_id == provider_id && slot.account_key.uuid == uuid {
        let title = format!("{} · {}", get_provider(provider_id).info.short_name, alias);
        #[cfg(desktop)]
        if let Some(main_win) = app.get_window("main") {
          let _ = main_win.set_title(&title);
        }
        #[cfg(target_os = "android")]
        {
          let _ = crate::platform::android::update_account_title(app, slot.switch_seq, &title).await;
        }
      }
    }

    Ok(Self::snapshot_from_locked(&locked))
  }

  pub async fn remove_account(
    &self,
    app: &AppHandle,
    raw_provider: &str,
    raw_uuid: &str,
  ) -> Result<ManagerSnapshot, String> {
    let provider_id = ProviderId::parse(raw_provider).ok_or_else(|| "无效的服务标识".to_string())?;
    let uuid = normalize_uuid(raw_uuid).ok_or_else(|| "无效的账号 UUID".to_string())?;
    let key = AccountKey {
      provider_id,
      uuid: uuid.clone(),
    };

    let closed_desktop_view = {
      let mut locked = self.inner.lock().await;
      if !locked.conflicts.is_empty() {
        return Err("存在 UUID 归属冲突，请先完成归属裁决".to_string());
      }
      let idx = locked
        .store
        .accounts
        .iter()
        .position(|a| a.provider_id == provider_id && a.uuid == uuid)
        .ok_or_else(|| "账号归属核对失败，未找到该账号".to_string())?;

      locked.store.accounts.remove(idx);
      if locked.store.last_committed.as_ref() == Some(&key) {
        locked.store.last_committed = None;
      }
      if locked.last_active_per_provider.get(&provider_id) == Some(&key) {
        locked.last_active_per_provider.remove(&provider_id);
      }
      if !locked.store.pending_cleanup.contains(&key) {
        locked.store.pending_cleanup.push(key.clone());
      }
      save_store_atomic(&locked.store_path, &locked.store)?;

      let active_matches = locked
        .active_slot
        .as_ref()
        .map(|s| s.account_key == key)
        .unwrap_or(false);
      if active_matches {
        let next_seq = self.current_seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.hide_foreground_slot(app, next_seq, &mut locked).await;
      }

      #[cfg(desktop)]
      {
        locked.prewarm_failed.remove(&key);
        if let Some(warm_entry) = locked.warm_pool.remove(&key) {
          Self::close_warm_entry(app, warm_entry);
          Self::apply_desktop_layout_locked(app, &locked);
          true
        } else {
          false
        }
      }
      #[cfg(not(desktop))]
      {
        false
      }
    };

    if closed_desktop_view {
      tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }

    if self.delete_profile_for_key(app, &uuid).await.is_ok() {
      let mut locked = self.inner.lock().await;
      locked.store.pending_cleanup.retain(|k| k != &key);
      let _ = save_store_atomic(&locked.store_path, &locked.store);
    }

    Ok(self.get_snapshot().await)
  }

  pub async fn set_modal_active(
    &self,
    app: &AppHandle,
    active: bool,
  ) -> Result<ManagerSnapshot, String> {
    let mut locked = self.inner.lock().await;
    locked.modal_active = active;
    #[cfg(desktop)]
    Self::apply_desktop_layout_locked(app, &locked);
    #[cfg(not(desktop))]
    let _ = app;
    Ok(Self::snapshot_from_locked(&locked))
  }

  pub async fn open_official_site(
    &self,
    app: &AppHandle,
    raw_provider: &str,
  ) -> Result<ManagerSnapshot, String> {
    let provider_id = ProviderId::parse(raw_provider).ok_or_else(|| "无效的服务标识".to_string())?;
    let homepage = get_provider(provider_id).info.homepage_url;

    #[cfg(desktop)]
    {
      let _ = app;
      platform::open_external_browser(homepage)?;
    }
    #[cfg(target_os = "android")]
    {
      crate::platform::android::open_external_url(app, homepage).await?;
    }

    Ok(self.get_snapshot().await)
  }

  pub async fn resolve_uuid_conflict(
    &self,
    keep_provider_raw: &str,
    raw_uuid: &str,
  ) -> Result<ManagerSnapshot, String> {
    let keep_provider =
      ProviderId::parse(keep_provider_raw).ok_or_else(|| "无效的服务标识".to_string())?;
    let uuid = normalize_uuid(raw_uuid).ok_or_else(|| "无效的账号 UUID".to_string())?;

    let mut locked = self.inner.lock().await;
    let mut kept_first = false;
    for acc in &mut locked.store.accounts {
      if acc.uuid == uuid {
        if acc.provider_id == keep_provider && !kept_first {
          kept_first = true;
        } else {
          acc.uuid = Uuid::new_v4().hyphenated().to_string();
        }
      }
    }
    locked.conflicts = detect_conflicts(&locked.store.accounts);
    if locked.conflicts.is_empty() {
      locked.notice = None;
    }
    save_store_atomic(&locked.store_path, &locked.store)?;
    Ok(Self::snapshot_from_locked(&locked))
  }

  pub async fn dismiss_notice(&self) -> ManagerSnapshot {
    let mut locked = self.inner.lock().await;
    locked.notice = None;
    Self::snapshot_from_locked(&locked)
  }

  pub async fn switch_account(
    self: &Arc<Self>,
    app: &AppHandle,
    raw_provider: &str,
    raw_uuid: &str,
  ) -> Result<ManagerSnapshot, String> {
    let provider_id = ProviderId::parse(raw_provider).ok_or_else(|| "无效的服务标识".to_string())?;
    let uuid = normalize_uuid(raw_uuid).ok_or_else(|| "无效的账号 UUID".to_string())?;
    let key = AccountKey { provider_id, uuid };
    self.switch_account_internal(app, key, false).await
  }

  pub async fn switch_account_by_index(
    self: &Arc<Self>,
    app: &AppHandle,
    index_1_based: u8,
  ) -> Result<ManagerSnapshot, String> {
    if index_1_based == 0 {
      return Ok(self.get_snapshot().await);
    }
    let (provider_id, target_uuid) = {
      let locked = self.inner.lock().await;
      let pid = locked.selected_provider_id;
      let accounts: Vec<_> = locked
        .store
        .accounts
        .iter()
        .filter(|a| a.provider_id == pid)
        .collect();
      let idx = (index_1_based - 1) as usize;
      if let Some(acc) = accounts.get(idx) {
        (pid, acc.uuid.clone())
      } else {
        return Ok(Self::snapshot_from_locked(&locked));
      }
    };
    self.switch_account(app, provider_id.as_str(), &target_uuid).await
  }

  pub async fn trigger_silent_sync(self: &Arc<Self>, app: &AppHandle, force: bool) {
    let now = now_ms();
    if !force {
      let last = self.last_sync_timestamp.load(Ordering::SeqCst);
      if now > last && now - last < 30_000 {
        return;
      }
    }
    self.last_sync_timestamp.store(now, Ordering::SeqCst);

    #[cfg(not(desktop))]
    {
      let _ = app;
    }

    #[cfg(desktop)]
    {
      let warm_targets = {
        let mut locked = self.inner.lock().await;
        if force {
          locked.prewarm_failed.clear();
        }
        locked
          .warm_pool
          .iter()
          .map(|(k, v)| (k.clone(), v.webview_label.clone(), v.view_instance_id))
          .collect::<Vec<_>>()
      };

      for (key, wv_label, vid) in warm_targets {
        if let Some(wv) = app.get_webview(&wv_label) {
          let _ = wv.eval(r#"(() => {
            try {
              if (typeof window.__maamRefreshBackendQuota === 'function') {
                window.__maamRefreshBackendQuota(true);
              }
            } catch (_) {}
          })()"#);

          let mgr = Arc::clone(self);
          let app_handle = app.clone();
          tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(850)).await;
            if let Some(probe) = Self::eval_identity_probe(&wv).await {
              if probe.is_auth {
                let resolved_identity = if !probe.email.is_empty() {
                  Some(probe.email)
                } else if !probe.typed.is_empty() {
                  Some(probe.typed)
                } else {
                  None
                };
                mgr
                  .apply_detected_metadata(
                    &app_handle,
                    key,
                    vid,
                    resolved_identity,
                    if probe.avatar.is_empty() {
                      None
                    } else {
                      Some(probe.avatar)
                    },
                    if probe.quota_text.is_empty() {
                      None
                    } else {
                      Some(probe.quota_text)
                    },
                    probe.quota_pct,
                    probe.quota_detail,
                  )
                  .await;
              } else if probe.logged_out {
                mgr.apply_logged_out_state(&app_handle, key, vid).await;
              }
            }
          });
        }
      }

      let mgr_prewarm = Arc::clone(self);
      let app_prewarm = app.clone();
      tauri::async_runtime::spawn(async move {
        mgr_prewarm.prewarm_next_cold_account(&app_prewarm).await;
      });
    }
  }

  async fn switch_account_internal(
    self: &Arc<Self>,
    app: &AppHandle,
    key: AccountKey,
    is_cold_start: bool,
  ) -> Result<ManagerSnapshot, String> {
    {
      let locked = self.inner.lock().await;
      if let Some(slot) = &locked.active_slot {
        if slot.committed && slot.account_key == key && !is_cold_start {
          #[cfg(desktop)]
          return Ok(Self::snapshot_from_locked(&locked));
        }
      }
    }

    let next_seq = self.current_seq.fetch_add(1, Ordering::SeqCst) + 1;

    #[cfg(desktop)]
    {
      enum DesktopSwitchPlan {
        InstantWarm {
          view_instance_id: u64,
          need_identity_watcher: bool,
        },
        AwaitWarmLoading {
          view_instance_id: u64,
        },
        CreateCold {
          account: AccountRecord,
          app_data_dir: PathBuf,
          view_instance_id: u64,
        },
        Abort(ManagerSnapshot),
      }

      let plan = {
        let mut locked = self.inner.lock().await;
        if !locked.conflicts.is_empty() {
          return Err("存在 UUID 归属冲突，已停留在管理页".to_string());
        }
        if !get_provider(key.provider_id).info.enabled {
          return Err(format!("{} 尚未开放应用内嵌", get_provider(key.provider_id).info.name));
        }
        if locked.store.pending_cleanup.contains(&key) {
          self.hide_foreground_slot(app, next_seq, &mut locked).await;
          locked.notice = Some("该账号正在清理本地资料，请稍后重试或重新创建".to_string());
          DesktopSwitchPlan::Abort(Self::snapshot_from_locked(&locked))
        } else {
          let Some(acc) = locked
            .store
            .accounts
            .iter()
            .find(|a| a.provider_id == key.provider_id && a.uuid == key.uuid)
            .cloned()
          else {
            self.hide_foreground_slot(app, next_seq, &mut locked).await;
            locked.notice = Some("账号归属核对失败，已回到管理页".to_string());
            return Ok(Self::snapshot_from_locked(&locked));
          };

          locked.selected_provider_id = key.provider_id;
          locked.notice = None;
          locked.prewarm_failed.remove(&key);
          self.hide_foreground_slot(app, next_seq, &mut locked).await;

          let warm_state = locked.warm_pool.get(&key).and_then(|entry| {
            if app.get_webview(&entry.webview_label).is_some() {
              Some((entry.view_instance_id, entry.paint_ready, entry.watcher_running))
            } else {
              None
            }
          });

          if let Some((vid, paint_ready, watcher_running)) = warm_state {
            if paint_ready {
              let prev_last = locked.store.last_committed.clone();
              locked.store.last_committed = Some(key.clone());
              if let Err(err) = save_store_atomic(&locked.store_path, &locked.store) {
                locked.store.last_committed = prev_last;
                locked.notice = Some(format!("提交账号记录写盘失败: {err}"));
                DesktopSwitchPlan::Abort(Self::snapshot_from_locked(&locked))
              } else {
                locked
                  .last_active_per_provider
                  .insert(key.provider_id, key.clone());
                locked.active_slot = Some(ActiveSessionSlot {
                  account_key: key.clone(),
                  view_instance_id: vid,
                  switch_seq: next_seq,
                  committed: true,
                });
                self.active_view_id.store(vid, Ordering::SeqCst);
                let title = format!(
                  "{} · {}",
                  get_provider(key.provider_id).info.short_name,
                  acc.alias
                );
                if let Some(main_win) = app.get_window("main") {
                  let _ = main_win.set_title(&title);
                }
                let need_watcher = !watcher_running;
                if let Some(entry) = locked.warm_pool.get_mut(&key) {
                  entry.last_used_ms = now_ms();
                  if need_watcher {
                    entry.watcher_running = true;
                  }
                }
                Self::apply_desktop_layout_locked(app, &locked);
                DesktopSwitchPlan::InstantWarm {
                  view_instance_id: vid,
                  need_identity_watcher: need_watcher,
                }
              }
            } else {
              if let Some(entry) = locked.warm_pool.get_mut(&key) {
                entry.last_used_ms = now_ms();
              }
              locked.active_slot = Some(ActiveSessionSlot {
                account_key: key.clone(),
                view_instance_id: vid,
                switch_seq: next_seq,
                committed: false,
              });
              self.active_view_id.store(vid, Ordering::SeqCst);
              Self::apply_desktop_layout_locked(app, &locked);
              DesktopSwitchPlan::AwaitWarmLoading {
                view_instance_id: vid,
              }
            }
          } else {
            locked.warm_pool.remove(&key);
            let vid = self.view_counter.fetch_add(1, Ordering::SeqCst);
            let dir = locked.app_data_dir.clone();
            DesktopSwitchPlan::CreateCold {
              account: acc,
              app_data_dir: dir,
              view_instance_id: vid,
            }
          }
        }
      };

      match plan {
        DesktopSwitchPlan::Abort(snap) => Ok(snap),
        DesktopSwitchPlan::InstantWarm {
          view_instance_id,
          need_identity_watcher,
        } => {
          if need_identity_watcher {
            self.spawn_desktop_identity_watcher(app.clone(), key, view_instance_id);
          }
          Ok(self.get_snapshot().await)
        }
        DesktopSwitchPlan::AwaitWarmLoading { view_instance_id } => {
          let _ = view_instance_id;
          Ok(self.get_snapshot().await)
        }
        DesktopSwitchPlan::CreateCold {
          account,
          app_data_dir,
          view_instance_id,
        } => {
          self
            .open_desktop_session(
              app,
              account,
              app_data_dir,
              next_seq,
              view_instance_id,
              is_cold_start,
            )
            .await
        }
      }
    }

    #[cfg(target_os = "android")]
    {
      let view_instance_id = self.view_counter.fetch_add(1, Ordering::SeqCst);
      let account_record = {
        let mut locked = self.inner.lock().await;
        if !locked.conflicts.is_empty() {
          return Err("存在 UUID 归属冲突，已停留在管理页".to_string());
        }
        if !get_provider(key.provider_id).info.enabled {
          return Err(format!("{} 尚未开放应用内嵌", get_provider(key.provider_id).info.name));
        }
        if locked.store.pending_cleanup.contains(&key) {
          self.hide_foreground_slot(app, next_seq, &mut locked).await;
          locked.notice = Some("该账号正在清理本地资料，请稍后重试或重新创建".to_string());
          return Ok(Self::snapshot_from_locked(&locked));
        }
        let Some(acc) = locked
          .store
          .accounts
          .iter()
          .find(|a| a.provider_id == key.provider_id && a.uuid == key.uuid)
          .cloned()
        else {
          self.hide_foreground_slot(app, next_seq, &mut locked).await;
          locked.notice = Some("账号归属核对失败，已回到管理页".to_string());
          return Ok(Self::snapshot_from_locked(&locked));
        };
        locked.selected_provider_id = key.provider_id;
        locked.notice = None;
        self.hide_foreground_slot(app, next_seq, &mut locked).await;
        acc
      };

      self
        .open_android_session(app, account_record, next_seq, view_instance_id, is_cold_start)
        .await
    }
  }

  #[cfg(desktop)]
  #[allow(clippy::too_many_arguments)]
  fn build_desktop_account_webview(
    self: &Arc<Self>,
    app: &AppHandle,
    key: AccountKey,
    parsed_uuid: Uuid,
    profile_dir: &std::path::Path,
    webview_label: &str,
    view_instance_id: u64,
    allow_blank: Arc<AtomicBool>,
    alive_flag: Arc<AtomicBool>,
  ) -> tauri::webview::WebviewBuilder<tauri::Wry> {
    let alive_nav = Arc::clone(&alive_flag);
    let mgr_for_nav = Arc::clone(self);
    let app_for_nav = app.clone();
    let allow_blank_nav = Arc::clone(&allow_blank);
    let provider_id = key.provider_id;
    let key_for_nav = key.clone();

    let alive_popup = Arc::clone(&alive_flag);
    let active_vid_popup = Arc::clone(&self.active_view_id);
    let mgr_for_popup = Arc::clone(self);
    let app_for_popup = app.clone();
    let key_for_popup = key.clone();
    let popup_counter = Arc::new(AtomicU64::new(1));

    let alive_load = Arc::clone(&alive_flag);
    let mgr_for_load = Arc::clone(self);
    let app_for_load = app.clone();
    let key_for_load = key;

    let mut builder = tauri::webview::WebviewBuilder::new(
      webview_label,
      tauri::WebviewUrl::External("about:blank".parse().expect("valid blank url")),
    )
    .focused(false)
    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
    .initialization_script(LOGIN_CAPTURE_INIT_JS)
    .on_navigation(move |url| {
      if !alive_nav.load(Ordering::SeqCst) {
        return false;
      }
      let can_blank = allow_blank_nav.load(Ordering::SeqCst);
      match classify_top_level_url(provider_id, url, can_blank) {
        NavigationDecision::AllowBlank => true,
        NavigationDecision::AllowHttps => true,
        NavigationDecision::ExternalGoogleOAuth => {
          let mgr_ext = Arc::clone(&mgr_for_nav);
          let app_ext = app_for_nav.clone();
          let key_ext = key_for_nav.clone();
          tauri::async_runtime::spawn(async move {
            mgr_ext
              .notify_external_auth_policy(&app_ext, key_ext, view_instance_id)
              .await;
          });
          false
        }
        NavigationDecision::Deny => false,
      }
    })
    .on_new_window(move |url, features| {
      if !alive_popup.load(Ordering::SeqCst)
        || active_vid_popup.load(Ordering::SeqCst) != view_instance_id
      {
        return tauri::webview::NewWindowResponse::Deny;
      }
      let decision = classify_top_level_url(provider_id, &url, true);
      match decision {
        NavigationDecision::ExternalGoogleOAuth => {
          let mgr_ext = Arc::clone(&mgr_for_popup);
          let app_ext = app_for_popup.clone();
          let key_ext = key_for_popup.clone();
          tauri::async_runtime::spawn(async move {
            mgr_ext
              .notify_external_auth_policy(&app_ext, key_ext, view_instance_id)
              .await;
          });
          return tauri::webview::NewWindowResponse::Deny;
        }
        NavigationDecision::Deny => {
          return tauri::webview::NewWindowResponse::Deny;
        }
        NavigationDecision::AllowBlank | NavigationDecision::AllowHttps => {}
      }

      let popup_idx = popup_counter.fetch_add(1, Ordering::SeqCst);
      let popup_label = format!("auth-popup-{view_instance_id}-{popup_idx}");
      let popup_label_for_close = popup_label.clone();
      let popup_label_for_slot = popup_label.clone();

      let alive_popup_nav = Arc::clone(&alive_popup);
      let active_vid_popup_nav = Arc::clone(&active_vid_popup);
      let app_popup_nav = app_for_popup.clone();
      let mgr_popup_nav = Arc::clone(&mgr_for_popup);
      let key_popup_nav = key_for_popup.clone();

      let popup_title = url
        .host_str()
        .map(|h| format!("登录认证 · {h}"))
        .unwrap_or_else(|| "登录认证".to_string());

      let popup_builder = tauri::WebviewWindowBuilder::new(
        &app_for_popup,
        &popup_label,
        tauri::WebviewUrl::External("about:blank".parse().expect("valid blank url")),
      )
      .window_features(features)
      .initialization_script(LOGIN_CAPTURE_INIT_JS)
      .title(&popup_title)
      .on_navigation(move |nav_url| {
        if !alive_popup_nav.load(Ordering::SeqCst)
          || active_vid_popup_nav.load(Ordering::SeqCst) != view_instance_id
        {
          if let Some(pw) = app_popup_nav.get_webview_window(&popup_label_for_close) {
            let _ = pw.close();
          }
          return false;
        }
        match classify_top_level_url(provider_id, nav_url, true) {
          NavigationDecision::AllowBlank => true,
          NavigationDecision::AllowHttps => {
            if let Some(host) = nav_url.host_str() {
              if let Some(pw) = app_popup_nav.get_webview_window(&popup_label_for_close) {
                let _ = pw.set_title(&format!("登录认证 · {host}"));
              }
            }
            true
          }
          NavigationDecision::ExternalGoogleOAuth => {
            if let Some(pw) = app_popup_nav.get_webview_window(&popup_label_for_close) {
              let _ = pw.close();
            }
            let mgr_ext = Arc::clone(&mgr_popup_nav);
            let app_ext = app_popup_nav.clone();
            let key_ext = key_popup_nav.clone();
            tauri::async_runtime::spawn(async move {
              mgr_ext
                .notify_external_auth_policy(&app_ext, key_ext, view_instance_id)
                .await;
            });
            false
          }
          NavigationDecision::Deny => false,
        }
      });

      match popup_builder.build() {
        Ok(popup_window) => {
          let mgr_slot = Arc::clone(&mgr_for_popup);
          let app_slot = app_for_popup.clone();
          let key_slot = key_for_popup.clone();
          tauri::async_runtime::spawn(async move {
            mgr_slot
              .register_desktop_popup(&app_slot, key_slot, view_instance_id, popup_label_for_slot)
              .await;
          });
          tauri::webview::NewWindowResponse::Create {
            window: popup_window,
          }
        }
        Err(_) => tauri::webview::NewWindowResponse::Deny,
      }
    })
    .on_page_load(move |_wv, payload| {
      if payload.event() != tauri::webview::PageLoadEvent::Finished {
        return;
      }
      if payload.url().as_str() == "about:blank" {
        return;
      }
      if !alive_load.load(Ordering::SeqCst) {
        return;
      }
      let loaded_url = payload.url().clone();
      let mgr_commit = Arc::clone(&mgr_for_load);
      let app_commit = app_for_load.clone();
      let key_commit = key_for_load.clone();
      tauri::async_runtime::spawn(async move {
        mgr_commit
          .wait_paint_and_finalize_desktop(&app_commit, key_commit, view_instance_id, loaded_url)
          .await;
      });
    });

    #[cfg(target_os = "macos")]
    {
      let _ = profile_dir;
      builder = builder
        .data_store_identifier(*parsed_uuid.as_bytes())
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15");
    }

    #[cfg(not(target_os = "macos"))]
    {
      let _ = parsed_uuid;
      builder = builder.data_directory(profile_dir.to_path_buf());
    }

    if provider_id == ProviderId::Hark {
      builder = builder.background_color(tauri::webview::Color(23, 29, 47, 255));
    }

    builder
  }

  #[cfg(desktop)]
  fn initial_hidden_child_rect(
    main_win: &tauri::Window,
  ) -> (tauri::LogicalPosition<f64>, tauri::LogicalSize<f64>) {
    let (win_w, win_h) = match (main_win.inner_size(), main_win.scale_factor()) {
      (Ok(phys), Ok(scale)) if scale > 0.0 => {
        ((phys.width as f64) / scale, (phys.height as f64) / scale)
      }
      _ => (1280.0, 800.0),
    };
    let left_w = win_w.min(LEFT_PANE_WIDTH);
    let right_w = (win_w - left_w).max(600.0);
    (
      tauri::LogicalPosition::new(win_w.max(2000.0) + 1000.0, 0.0),
      tauri::LogicalSize::new(right_w, win_h.max(400.0)),
    )
  }

  #[cfg(desktop)]
  async fn open_desktop_session(
    self: &Arc<Self>,
    app: &AppHandle,
    account: AccountRecord,
    app_data_dir: PathBuf,
    expected_seq: u64,
    view_instance_id: u64,
    is_cold_start: bool,
  ) -> Result<ManagerSnapshot, String> {
    let parsed_uuid = Uuid::parse_str(&account.uuid).map_err(|e| format!("非法 UUID: {e}"))?;
    let profile_dir = app_data_dir.join("profiles").join(&account.uuid);

    #[cfg(windows)]
    if is_cold_start && !profile_dir.exists() {
      let mut locked = self.inner.lock().await;
      locked.notice = Some("上次已提交账号的 Windows WebView2 资料目录缺失，已停留在管理页".to_string());
      return Ok(Self::snapshot_from_locked(&locked));
    }
    #[cfg(not(windows))]
    let _ = is_cold_start;

    #[cfg(windows)]
    std::fs::create_dir_all(&profile_dir).map_err(|e| format!("创建资料目录失败: {e}"))?;

    let main_win = app
      .get_window("main")
      .ok_or_else(|| "主窗口不存在".to_string())?;

    let webview_label = format!("account-{view_instance_id}");
    let allow_blank = Arc::new(AtomicBool::new(true));
    let alive_flag = Arc::new(AtomicBool::new(true));
    let key = AccountKey {
      provider_id: account.provider_id,
      uuid: account.uuid.clone(),
    };

    {
      let mut locked = self.inner.lock().await;
      if self.current_seq.load(Ordering::SeqCst) != expected_seq {
        return Ok(Self::snapshot_from_locked(&locked));
      }
      locked.active_slot = Some(ActiveSessionSlot {
        account_key: key.clone(),
        view_instance_id,
        switch_seq: expected_seq,
        committed: false,
      });
      self.active_view_id.store(view_instance_id, Ordering::SeqCst);
      locked.warm_pool.insert(
        key.clone(),
        WarmDesktopEntry {
          account_key: key.clone(),
          view_instance_id,
          webview_label: webview_label.clone(),
          popup_label: None,
          paint_ready: false,
          last_url: None,
          captured_identity_hint: None,
          watcher_running: false,
          last_used_ms: now_ms(),
          alive_flag: Arc::clone(&alive_flag),
        },
      );
      Self::evict_warm_pool_if_needed(app, &mut locked, Some(&key), MAX_WARM_DESKTOP_VIEWS);
    }

    let builder = self.build_desktop_account_webview(
      app,
      key.clone(),
      parsed_uuid,
      &profile_dir,
      &webview_label,
      view_instance_id,
      allow_blank,
      Arc::clone(&alive_flag),
    );

    let (init_pos, init_size) = Self::initial_hidden_child_rect(&main_win);
    let created_wv = match main_win.add_child(builder, init_pos, init_size) {
      Ok(wv) => wv,
      Err(err) => {
        alive_flag.store(false, Ordering::SeqCst);
        let mut locked = self.inner.lock().await;
        locked.warm_pool.remove(&key);
        if self.current_seq.load(Ordering::SeqCst) == expected_seq {
          locked.active_slot = None;
          self.active_view_id.store(0, Ordering::SeqCst);
          locked.notice = Some(format!("创建独立账号视图失败: {err}"));
          Self::apply_desktop_layout_locked(app, &locked);
        }
        return Ok(Self::snapshot_from_locked(&locked));
      }
    };
    let _ = created_wv.hide();

    if let Err(verify_err) =
      platform::verify_desktop_profile_binding(&created_wv, parsed_uuid, &profile_dir).await
    {
      alive_flag.store(false, Ordering::SeqCst);
      let _ = created_wv.close();
      let mut locked = self.inner.lock().await;
      locked.warm_pool.remove(&key);
      if self.current_seq.load(Ordering::SeqCst) == expected_seq {
        locked.active_slot = None;
        self.active_view_id.store(0, Ordering::SeqCst);
        locked.notice = Some(verify_err);
        Self::apply_desktop_layout_locked(app, &locked);
      }
      return Ok(Self::snapshot_from_locked(&locked));
    }

    if !alive_flag.load(Ordering::SeqCst) {
      let _ = created_wv.close();
      return Ok(self.get_snapshot().await);
    }

    let entry_url: Url = get_provider(account.provider_id)
      .info
      .entry_url
      .parse()
      .map_err(|e| format!("服务入口地址错误: {e}"))?;

    if let Err(nav_err) = created_wv.navigate(entry_url) {
      alive_flag.store(false, Ordering::SeqCst);
      let _ = created_wv.close();
      let mut locked = self.inner.lock().await;
      locked.warm_pool.remove(&key);
      if self.current_seq.load(Ordering::SeqCst) == expected_seq {
        locked.active_slot = None;
        self.active_view_id.store(0, Ordering::SeqCst);
        locked.notice = Some(format!("导航到服务入口失败: {nav_err}"));
        Self::apply_desktop_layout_locked(app, &locked);
      }
      return Ok(Self::snapshot_from_locked(&locked));
    }

    Ok(self.get_snapshot().await)
  }

  #[cfg(desktop)]
  async fn eval_paint_ready(webview: &tauri::Webview) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx_cell = Arc::new(std::sync::Mutex::new(Some(tx)));
    let tx_inner = Arc::clone(&tx_cell);
    if webview
      .eval_with_callback(DOM_PAINT_READY_JS, move |raw| {
        if let Some(sender) = tx_inner.lock().unwrap().take() {
          let _ = sender.send(raw);
        }
      })
      .is_err()
    {
      return false;
    }

    match tokio::time::timeout(std::time::Duration::from_millis(600), rx).await {
      Ok(Ok(raw)) => raw.trim() == "true",
      _ => false,
    }
  }

  #[cfg(desktop)]
  async fn wait_paint_and_finalize_desktop(
    self: &Arc<Self>,
    app: &AppHandle,
    key: AccountKey,
    view_instance_id: u64,
    loaded_url: Url,
  ) {
    if classify_top_level_url(key.provider_id, &loaded_url, false) != NavigationDecision::AllowHttps {
      return;
    }

    let wv_label_opt = {
      let locked = self.inner.lock().await;
      locked.warm_pool.get(&key).and_then(|e| {
        if e.view_instance_id == view_instance_id {
          Some((e.webview_label.clone(), e.paint_ready))
        } else {
          None
        }
      })
    };

    let Some((wv_label, already_ready)) = wv_label_opt else {
      return;
    };

    if !already_ready {
      for _ in 0..45 {
        let Some(wv) = app.get_webview(&wv_label) else {
          return;
        };
        if Self::eval_paint_ready(&wv).await {
          tokio::time::sleep(std::time::Duration::from_millis(80)).await;
          break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
      }
    }

    let check_params = {
      let locked = self.inner.lock().await;
      locked.warm_pool.get(&key).and_then(|e| {
        if e.view_instance_id == view_instance_id {
          Some((
            e.webview_label.clone(),
            locked.app_data_dir.join("profiles").join(&key.uuid),
          ))
        } else {
          None
        }
      })
    };

    let Some((wv_label, expected_dir)) = check_params else {
      return;
    };
    let Ok(parsed_uuid) = Uuid::parse_str(&key.uuid) else {
      return;
    };
    let Some(wv) = app.get_webview(&wv_label) else {
      return;
    };

    if let Err(err) =
      platform::verify_desktop_profile_binding(&wv, parsed_uuid, &expected_dir).await
    {
      let snap = {
        let mut locked = self.inner.lock().await;
        if let Some(entry) = locked.warm_pool.remove(&key) {
          Self::close_warm_entry(app, entry);
        }
        let is_fg = locked
          .active_slot
          .as_ref()
          .map(|s| s.account_key == key && s.view_instance_id == view_instance_id)
          .unwrap_or(false);
        if is_fg {
          locked.active_slot = None;
          self.active_view_id.store(0, Ordering::SeqCst);
          locked.notice = Some(err);
          Self::apply_desktop_layout_locked(app, &locked);
        }
        Self::snapshot_from_locked(&locked)
      };
      self.emit_snapshot(app, &snap);
      return;
    }

    let (snap_opt, start_watcher) = {
      let mut locked = self.inner.lock().await;
      if !locked.conflicts.is_empty() || locked.store.pending_cleanup.contains(&key) {
        if let Some(entry) = locked.warm_pool.remove(&key) {
          Self::close_warm_entry(app, entry);
        }
        return;
      }
      let Some(acc) = locked
        .store
        .accounts
        .iter()
        .find(|a| a.provider_id == key.provider_id && a.uuid == key.uuid)
        .cloned()
      else {
        if let Some(entry) = locked.warm_pool.remove(&key) {
          Self::close_warm_entry(app, entry);
        }
        return;
      };

      let mut need_watcher = false;
      if let Some(entry) = locked.warm_pool.get_mut(&key) {
        if entry.view_instance_id != view_instance_id {
          return;
        }
        entry.paint_ready = true;
        entry.last_url = Some(loaded_url);
        if !entry.watcher_running {
          entry.watcher_running = true;
          need_watcher = true;
        }
      } else {
        return;
      }

      let is_foreground_waiting = locked
        .active_slot
        .as_ref()
        .map(|s| s.account_key == key && s.view_instance_id == view_instance_id && !s.committed)
        .unwrap_or(false);

      if is_foreground_waiting {
        let prev_last = locked.store.last_committed.clone();
        locked.store.last_committed = Some(key.clone());
        if let Err(err) = save_store_atomic(&locked.store_path, &locked.store) {
          locked.store.last_committed = prev_last;
          locked.active_slot = None;
          self.active_view_id.store(0, Ordering::SeqCst);
          locked.notice = Some(format!("提交账号记录写盘失败，已回退管理页: {err}"));
          Self::apply_desktop_layout_locked(app, &locked);
          (Some(Self::snapshot_from_locked(&locked)), false)
        } else {
          if let Some(slot) = &mut locked.active_slot {
            slot.committed = true;
          }
          if let Some(entry) = locked.warm_pool.get_mut(&key) {
            entry.last_used_ms = now_ms();
          }
          locked
            .last_active_per_provider
            .insert(key.provider_id, key.clone());
          let title = format!(
            "{} · {}",
            get_provider(key.provider_id).info.short_name,
            acc.alias
          );
          if let Some(main_win) = app.get_window("main") {
            let _ = main_win.set_title(&title);
          }
          Self::apply_desktop_layout_locked(app, &locked);
          (Some(Self::snapshot_from_locked(&locked)), need_watcher)
        }
      } else {
        (Some(Self::snapshot_from_locked(&locked)), need_watcher)
      }
    };

    if start_watcher {
      self.spawn_desktop_identity_watcher(app.clone(), key, view_instance_id);
    }

    if let Some(snap) = snap_opt {
      self.emit_snapshot(app, &snap);
    }

    let mgr_next = Arc::clone(self);
    let app_next = app.clone();
    tauri::async_runtime::spawn(async move {
      tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
      mgr_next.prewarm_next_cold_account(&app_next).await;
    });
  }

  #[cfg(desktop)]
  async fn prewarm_desktop_account(
    self: &Arc<Self>,
    app: &AppHandle,
    account: AccountRecord,
  ) -> Result<(), String> {
    let key = AccountKey {
      provider_id: account.provider_id,
      uuid: account.uuid.clone(),
    };
    let app_data_dir = {
      let locked = self.inner.lock().await;
      if locked.warm_pool.contains_key(&key)
        || locked.store.pending_cleanup.contains(&key)
        || locked.warm_pool.len() >= MAX_WARM_DESKTOP_VIEWS
      {
        return Ok(());
      }
      locked.app_data_dir.clone()
    };

    let parsed_uuid = Uuid::parse_str(&account.uuid).map_err(|e| format!("非法 UUID: {e}"))?;
    let profile_dir = app_data_dir.join("profiles").join(&account.uuid);

    #[cfg(windows)]
    std::fs::create_dir_all(&profile_dir).map_err(|e| format!("创建资料目录失败: {e}"))?;

    let view_instance_id = self.view_counter.fetch_add(1, Ordering::SeqCst);
    let webview_label = format!("account-{view_instance_id}");
    let allow_blank = Arc::new(AtomicBool::new(true));
    let alive_flag = Arc::new(AtomicBool::new(true));

    {
      let mut locked = self.inner.lock().await;
      if locked.warm_pool.contains_key(&key) || locked.warm_pool.len() >= MAX_WARM_DESKTOP_VIEWS {
        return Ok(());
      }
      locked.warm_pool.insert(
        key.clone(),
        WarmDesktopEntry {
          account_key: key.clone(),
          view_instance_id,
          webview_label: webview_label.clone(),
          popup_label: None,
          paint_ready: false,
          last_url: None,
          captured_identity_hint: None,
          watcher_running: false,
          last_used_ms: 0,
          alive_flag: Arc::clone(&alive_flag),
        },
      );
    }

    let builder = self.build_desktop_account_webview(
      app,
      key.clone(),
      parsed_uuid,
      &profile_dir,
      &webview_label,
      view_instance_id,
      allow_blank,
      Arc::clone(&alive_flag),
    );

    let sync_res = (|| {
      let main_win = app.get_window("main").ok_or_else(|| "主窗口不存在".to_string())?;
      let (init_pos, init_size) = Self::initial_hidden_child_rect(&main_win);
      let created_wv = main_win
        .add_child(builder, init_pos, init_size)
        .map_err(|err| format!("创建预热视图失败: {err}"))?;
      let _ = created_wv.hide();
      Ok(platform::verify_desktop_profile_binding(
        &created_wv,
        parsed_uuid,
        &profile_dir,
      ))
    })();

    let verify_fut = match sync_res {
      Ok(fut) => fut,
      Err(err) => {
        alive_flag.store(false, Ordering::SeqCst);
        let mut locked = self.inner.lock().await;
        locked.warm_pool.remove(&key);
        return Err(err);
      }
    };

    if let Err(err) = verify_fut.await {
      alive_flag.store(false, Ordering::SeqCst);
      if let Some(wv) = app.get_webview(&webview_label) {
        let _ = wv.close();
      }
      let mut locked = self.inner.lock().await;
      locked.warm_pool.remove(&key);
      return Err(format!("验证预热隔离失败: {err}"));
    }

    if !alive_flag.load(Ordering::SeqCst) {
      if let Some(wv) = app.get_webview(&webview_label) {
        let _ = wv.close();
      }
      return Ok(());
    }

    let entry_url: Url = get_provider(account.provider_id)
      .info
      .entry_url
      .parse()
      .map_err(|e| format!("服务入口地址错误: {e}"))?;

    if let Some(wv) = app.get_webview(&webview_label) {
      let _ = wv.navigate(entry_url);
    }

    Ok(())
  }

  #[cfg(desktop)]
  pub fn prewarm_next_cold_account(
    self: &Arc<Self>,
    app: &AppHandle,
  ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    let mgr = Arc::clone(self);
    let app_handle = app.clone();
    Box::pin(async move {
      let next_cold = {
        let locked = mgr.inner.lock().await;
        if locked.warm_pool.len() >= MAX_WARM_DESKTOP_VIEWS {
          None
        } else {
          locked
            .store
            .accounts
            .iter()
            .filter(|a| get_provider(a.provider_id).info.enabled)
            .find(|a| {
              let k = AccountKey {
                provider_id: a.provider_id,
                uuid: a.uuid.clone(),
              };
              !locked.warm_pool.contains_key(&k)
                && !locked.store.pending_cleanup.contains(&k)
                && !locked.prewarm_failed.contains(&k)
            })
            .cloned()
        }
      };

      if let Some(account) = next_cold {
        let failed_key = AccountKey {
          provider_id: account.provider_id,
          uuid: account.uuid.clone(),
        };
        if mgr.prewarm_desktop_account(&app_handle, account).await.is_err() {
          {
            let mut locked = mgr.inner.lock().await;
            locked.prewarm_failed.insert(failed_key);
          }
          tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
          mgr.prewarm_next_cold_account(&app_handle).await;
        }
      }
    })
  }

  #[cfg(not(desktop))]
  pub fn prewarm_next_cold_account(
    self: &Arc<Self>,
    _app: &AppHandle,
  ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {})
  }

  #[cfg(desktop)]
  async fn register_desktop_popup(
    &self,
    app: &AppHandle,
    key: AccountKey,
    view_instance_id: u64,
    popup_label: String,
  ) {
    let mut locked = self.inner.lock().await;
    if self.active_view_id.load(Ordering::SeqCst) != view_instance_id {
      if let Some(pw) = app.get_webview_window(&popup_label) {
        let _ = pw.close();
      }
      return;
    }
    let Some(entry) = locked.warm_pool.get_mut(&key) else {
      if let Some(pw) = app.get_webview_window(&popup_label) {
        let _ = pw.close();
      }
      return;
    };
    if entry.view_instance_id != view_instance_id {
      if let Some(pw) = app.get_webview_window(&popup_label) {
        let _ = pw.close();
      }
      return;
    }
    if let Some(prev_popup) = entry.popup_label.replace(popup_label) {
      if let Some(pw) = app.get_webview_window(&prev_popup) {
        let _ = pw.close();
      }
    }
  }

  #[cfg(target_os = "android")]
  async fn open_android_session(
    self: &Arc<Self>,
    app: &AppHandle,
    account: AccountRecord,
    expected_seq: u64,
    view_instance_id: u64,
    is_cold_start: bool,
  ) -> Result<ManagerSnapshot, String> {
    let caps = match crate::platform::android::check_capabilities(app).await {
      Ok(c) => c,
      Err(err) => {
        let mut locked = self.inner.lock().await;
        locked.notice = Some(format!("检测 Android WebView 多资料能力失败: {err}"));
        return Ok(Self::snapshot_from_locked(&locked));
      }
    };

    if !caps.multi_profile_supported {
      let mut locked = self.inner.lock().await;
      locked.notice = Some(
        "当前设备系统 WebView 不支持 MULTI_PROFILE 多资料隔离，请先更新 Android System WebView 后重试"
          .to_string(),
      );
      return Ok(Self::snapshot_from_locked(&locked));
    }

    if is_cold_start && !caps.profile_names.iter().any(|n| n == &account.uuid) {
      let mut locked = self.inner.lock().await;
      locked.notice = Some("上次已提交账号的系统 Profile 已丢失，已停留在管理页".to_string());
      return Ok(Self::snapshot_from_locked(&locked));
    }

    let key = AccountKey {
      provider_id: account.provider_id,
      uuid: account.uuid.clone(),
    };

    {
      let mut locked = self.inner.lock().await;
      if self.current_seq.load(Ordering::SeqCst) != expected_seq {
        return Ok(Self::snapshot_from_locked(&locked));
      }
      locked.active_slot = Some(ActiveSessionSlot {
        account_key: key,
        view_instance_id,
        switch_seq: expected_seq,
        committed: false,
      });
    }

    let spec = get_provider(account.provider_id);
    let payload = crate::platform::android::OpenAndroidAccountPayload {
      provider_id: account.provider_id.as_str().to_string(),
      provider_name: spec.info.short_name.to_string(),
      uuid: account.uuid.clone(),
      alias: account.alias.clone(),
      auto_alias: account.auto_alias,
      entry_url: spec.info.entry_url.to_string(),
      allowed_hosts: spec.allowed_hosts.iter().map(|s| (*s).to_string()).collect(),
      switch_seq: expected_seq,
      view_instance_id,
      is_cold_start,
    };

    if let Err(err) = crate::platform::android::open_account_session(app, payload).await {
      let mut locked = self.inner.lock().await;
      if self.current_seq.load(Ordering::SeqCst) == expected_seq {
        locked.active_slot = None;
        locked.notice = Some(format!("打开 Android 独立会话页面失败: {err}"));
      }
      return Ok(Self::snapshot_from_locked(&locked));
    }

    Ok(self.get_snapshot().await)
  }

  #[cfg(target_os = "android")]
  async fn handle_android_event(
    self: &Arc<Self>,
    app: &AppHandle,
    event: crate::platform::android::AndroidSessionEvent,
  ) {
    use crate::platform::android::AndroidSessionEvent;
    match event {
      AndroidSessionEvent::PageCommitVisible {
        provider_id,
        uuid,
        switch_seq,
        view_instance_id,
        url,
      } => {
        let Some(pid) = ProviderId::parse(&provider_id) else {
          return;
        };
        let Some(norm_uuid) = normalize_uuid(&uuid) else {
          return;
        };
        let Ok(parsed_url) = Url::parse(&url) else {
          return;
        };
        self
          .commit_android_session(
            app,
            AccountKey {
              provider_id: pid,
              uuid: norm_uuid,
            },
            view_instance_id,
            switch_seq,
            parsed_url,
          )
          .await;
      }
      AndroidSessionEvent::IdentityDetected {
        provider_id,
        uuid,
        switch_seq,
        view_instance_id,
        identity,
        avatar_url,
        quota_text,
        quota_percent,
        quota_detail_json,
      } => {
        let Some(pid) = ProviderId::parse(&provider_id) else {
          return;
        };
        let Some(norm_uuid) = normalize_uuid(&uuid) else {
          return;
        };
        let _ = switch_seq;
        let parsed_detail = quota_detail_json
          .as_deref()
          .and_then(|s| serde_json::from_str::<QuotaDetail>(s).ok());
        self
          .apply_detected_metadata(
            app,
            AccountKey {
              provider_id: pid,
              uuid: norm_uuid,
            },
            view_instance_id,
            if identity.is_empty() { None } else { Some(identity) },
            avatar_url,
            quota_text,
            quota_percent,
            parsed_detail,
          )
          .await;
      }
      AndroidSessionEvent::SessionFailed {
        switch_seq,
        view_instance_id,
        reason,
      } => {
        let snap = {
          let mut locked = self.inner.lock().await;
          if self.current_seq.load(Ordering::SeqCst) != switch_seq {
            return;
          }
          let matches = locked
            .active_slot
            .as_ref()
            .map(|s| s.switch_seq == switch_seq && s.view_instance_id == view_instance_id)
            .unwrap_or(false);
          if !matches {
            return;
          }
          locked.active_slot = None;
          locked.notice = Some(reason);
          Self::snapshot_from_locked(&locked)
        };
        self.emit_snapshot(app, &snap);
      }
      AndroidSessionEvent::ActivityClosed {
        switch_seq,
        view_instance_id,
      } => {
        let snap = {
          let mut locked = self.inner.lock().await;
          let matches = locked
            .active_slot
            .as_ref()
            .map(|s| s.switch_seq == switch_seq && s.view_instance_id == view_instance_id)
            .unwrap_or(false);
          if !matches {
            return;
          }
          locked.active_slot = None;
          locked.pending_external_auth = None;
          Self::snapshot_from_locked(&locked)
        };
        self.emit_snapshot(app, &snap);
      }
      AndroidSessionEvent::ExternalAuthUnavailable {
        switch_seq,
        view_instance_id,
        reason,
      } => {
        let snap = {
          let mut locked = self.inner.lock().await;
          let matches = locked
            .active_slot
            .as_ref()
            .map(|s| s.switch_seq == switch_seq && s.view_instance_id == view_instance_id)
            .unwrap_or(false);
          if !matches {
            return;
          }
          locked.pending_external_auth = None;
          locked.notice = Some(reason);
          Self::snapshot_from_locked(&locked)
        };
        self.emit_snapshot(app, &snap);
      }
      AndroidSessionEvent::RecreateRequested { previous_switch_seq } => {
        let fallback = {
          let mut locked = self.inner.lock().await;
          if self.current_seq.load(Ordering::SeqCst) == previous_switch_seq {
            locked.active_slot = None;
          }
          locked.store.last_committed.clone()
        };
        if let Some(committed_key) = fallback {
          let _ = self.switch_account_internal(app, committed_key, true).await;
        } else {
          let snap = self.get_snapshot().await;
          self.emit_snapshot(app, &snap);
        }
      }
      AndroidSessionEvent::UpdateProgress {
        stage,
        downloaded,
        total,
        percent,
        message,
      } => {
        let _ = app.emit(
          "update-progress",
          crate::updater::UpdateProgressEvent {
            stage,
            downloaded,
            total,
            percent,
            message,
          },
        );
      }
      AndroidSessionEvent::SessionLoggedOut {
        provider_id,
        uuid,
        switch_seq,
        view_instance_id,
      } => {
        let _ = switch_seq;
        let Some(pid) = ProviderId::parse(&provider_id) else {
          return;
        };
        let Some(norm_uuid) = normalize_uuid(&uuid) else {
          return;
        };
        self
          .apply_logged_out_state(
            app,
            AccountKey {
              provider_id: pid,
              uuid: norm_uuid,
            },
            view_instance_id,
          )
          .await;
      }
    }
  }

  #[cfg(target_os = "android")]
  async fn commit_android_session(
    &self,
    app: &AppHandle,
    key: AccountKey,
    view_instance_id: u64,
    expected_seq: u64,
    committed_url: Url,
  ) {
    if self.current_seq.load(Ordering::SeqCst) != expected_seq {
      return;
    }
    if classify_top_level_url(key.provider_id, &committed_url, false) != NavigationDecision::AllowHttps {
      return;
    }

    let snap_opt = {
      let mut locked = self.inner.lock().await;
      if self.current_seq.load(Ordering::SeqCst) != expected_seq {
        return;
      }
      if !locked.conflicts.is_empty() || locked.store.pending_cleanup.contains(&key) {
        self.hide_foreground_slot(app, expected_seq, &mut locked).await;
        return;
      }
      let Some(acc) = locked
        .store
        .accounts
        .iter()
        .find(|a| a.provider_id == key.provider_id && a.uuid == key.uuid)
        .cloned()
      else {
        self.hide_foreground_slot(app, expected_seq, &mut locked).await;
        return;
      };

      let slot_matches = locked
        .active_slot
        .as_ref()
        .map(|s| {
          s.account_key == key
            && s.view_instance_id == view_instance_id
            && s.switch_seq == expected_seq
        })
        .unwrap_or(false);
      if !slot_matches {
        return;
      }

      let prev_last = locked.store.last_committed.clone();
      locked.store.last_committed = Some(key.clone());
      if let Err(err) = save_store_atomic(&locked.store_path, &locked.store) {
        locked.store.last_committed = prev_last;
        self.hide_foreground_slot(app, expected_seq, &mut locked).await;
        locked.notice = Some(format!("提交账号记录写盘失败，已回退管理页: {err}"));
        Some(Self::snapshot_from_locked(&locked))
      } else {
        if let Some(slot) = &mut locked.active_slot {
          slot.committed = true;
        }
        let title = format!(
          "{} · {}",
          get_provider(key.provider_id).info.short_name,
          acc.alias
        );
        let _ = crate::platform::android::update_account_title(app, expected_seq, &title).await;
        Some(Self::snapshot_from_locked(&locked))
      }
    };

    if let Some(snap) = snap_opt {
      self.emit_snapshot(app, &snap);
    }
  }

  #[cfg(desktop)]
  async fn notify_external_auth_policy(
    &self,
    app: &AppHandle,
    key: AccountKey,
    view_instance_id: u64,
  ) {
    let snap_opt = {
      let mut locked = self.inner.lock().await;
      let matches = locked
        .active_slot
        .as_ref()
        .map(|s| s.account_key == key && s.view_instance_id == view_instance_id)
        .unwrap_or(false);
      if !matches {
        return;
      }
      let cur_seq = self.current_seq.load(Ordering::SeqCst);
      let entry_ok = Url::parse(get_provider(key.provider_id).info.entry_url)
        .map(|u| is_verified_callback_origin(key.provider_id, &u))
        .unwrap_or(false);
      locked.pending_external_auth = Some(PendingExternalAuth {
        account_key: key.clone(),
        switch_seq: cur_seq,
        nonce: format!("{}-{}", key.provider_id.as_str(), Uuid::new_v4().simple()),
      });
      if !entry_ok || locked.pending_external_auth.is_some() {
        locked.pending_external_auth = None;
      }
      locked.notice = Some(
        "Google OAuth 按政策禁止内嵌 WebView，且系统浏览器回跳暂无法绑定到当前独立浏览资料，请使用邮箱、手机号或已验收方式登录"
          .to_string(),
      );
      Some(Self::snapshot_from_locked(&locked))
    };

    if let Some(snap) = snap_opt {
      self.emit_snapshot(app, &snap);
    }
  }

  #[cfg(desktop)]
  fn spawn_desktop_identity_watcher(
    self: &Arc<Self>,
    app: AppHandle,
    key: AccountKey,
    view_instance_id: u64,
  ) {
    let mgr = Arc::clone(self);
    tauri::async_runtime::spawn(async move {
      let mut tick_count: u32 = 0;
      let mut consecutive_logged_out: u8 = 0;
      loop {
        let delay_ms = if tick_count < 6 { 950 } else { 6500 };
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        tick_count = tick_count.saturating_add(1);

        let labels_opt = {
          let locked = mgr.inner.lock().await;
          let account_exists = locked
            .store
            .accounts
            .iter()
            .any(|a| a.provider_id == key.provider_id && a.uuid == key.uuid);
          if !account_exists {
            None
          } else {
            locked.warm_pool.get(&key).and_then(|e| {
              if e.view_instance_id == view_instance_id && e.alive_flag.load(Ordering::SeqCst) {
                Some((e.webview_label.clone(), e.popup_label.clone()))
              } else {
                None
              }
            })
          }
        };

        let Some((wv_label, popup_label)) = labels_opt else {
          let mut locked = mgr.inner.lock().await;
          if let Some(entry) = locked.warm_pool.get_mut(&key) {
            if entry.view_instance_id == view_instance_id {
              entry.watcher_running = false;
            }
          }
          break;
        };

        if let Some(p_label) = popup_label {
          if let Some(p_wv) = app.get_webview(&p_label) {
            if let Some(probe) = Self::eval_identity_probe(&p_wv).await {
              if !probe.typed.is_empty() {
                let mut locked = mgr.inner.lock().await;
                if let Some(entry) = locked.warm_pool.get_mut(&key) {
                  if entry.view_instance_id == view_instance_id {
                    entry.captured_identity_hint = Some(probe.typed);
                  }
                }
              }
            }
          }
        }

        let Some(main_wv) = app.get_webview(&wv_label) else {
          let mut locked = mgr.inner.lock().await;
          if let Some(entry) = locked.warm_pool.get_mut(&key) {
            if entry.view_instance_id == view_instance_id {
              entry.watcher_running = false;
            }
          }
          break;
        };
        let Some(probe) = Self::eval_identity_probe(&main_wv).await else {
          continue;
        };

        if let Some(idx) = probe.pending_hotkey {
          let mgr_hotkey = Arc::clone(&mgr);
          let app_hotkey = app.clone();
          tauri::async_runtime::spawn(async move {
            let _ = mgr_hotkey.switch_account_by_index(&app_hotkey, idx).await;
          });
        }

        let resolved_identity = {
          let mut locked = mgr.inner.lock().await;
          if let Some(entry) = locked.warm_pool.get_mut(&key) {
            if entry.view_instance_id == view_instance_id {
              if !probe.typed.is_empty() {
                entry.captured_identity_hint = Some(probe.typed);
              }
              if probe.is_auth {
                if !probe.email.is_empty() {
                  Some(probe.email)
                } else {
                  entry.captured_identity_hint.clone()
                }
              } else {
                None
              }
            } else {
              None
            }
          } else {
            None
          }
        };

        if probe.is_auth {
          consecutive_logged_out = 0;
          mgr
            .apply_detected_metadata(
              &app,
              key.clone(),
              view_instance_id,
              resolved_identity,
              if probe.avatar.is_empty() {
                None
              } else {
                Some(probe.avatar)
              },
              if probe.quota_text.is_empty() {
                None
              } else {
                Some(probe.quota_text)
              },
              probe.quota_pct,
              probe.quota_detail,
            )
            .await;
        } else if probe.logged_out {
          consecutive_logged_out = consecutive_logged_out.saturating_add(1);
          if consecutive_logged_out >= 2 {
            mgr
              .apply_logged_out_state(&app, key.clone(), view_instance_id)
              .await;
          }
        } else {
          consecutive_logged_out = 0;
        }
      }
    });
  }

  #[cfg(desktop)]
  async fn eval_identity_probe(webview: &tauri::Webview) -> Option<DesktopProbeResult> {
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx_cell = Arc::new(std::sync::Mutex::new(Some(tx)));
    let tx_inner = Arc::clone(&tx_cell);
    webview
      .eval_with_callback(IDENTITY_PROBE_JS, move |raw| {
        if let Some(sender) = tx_inner.lock().unwrap().take() {
          let _ = sender.send(raw);
        }
      })
      .ok()?;

    let raw = tokio::time::timeout(std::time::Duration::from_millis(1200), rx)
      .await
      .ok()?
      .ok()?;

    let inner_json = serde_json::from_str::<String>(&raw).ok()?;
    let val = serde_json::from_str::<serde_json::Value>(&inner_json).ok()?;
    let is_auth = val.get("auth").and_then(|v| v.as_bool()).unwrap_or(false);
    let logged_out = val.get("loggedOut").and_then(|v| v.as_bool()).unwrap_or(false);
    let typed = val
      .get("typed")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .trim()
      .to_string();
    let email = val
      .get("email")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .trim()
      .to_string();
    let avatar = val
      .get("avatar")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .trim()
      .to_string();
    let quota_text = val
      .get("quotaText")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .trim()
      .to_string();
    let quota_pct = val
      .get("quotaPct")
      .and_then(|v| v.as_u64())
      .map(|n| n.min(100) as u8);
    let quota_detail = val
      .get("quotaDetail")
      .filter(|v| !v.is_null())
      .and_then(|v| serde_json::from_value::<QuotaDetail>(v.clone()).ok())
      .filter(|d| !d.plan_label.trim().is_empty() || !d.plan_value.trim().is_empty());
    let pending_hotkey = val
      .get("pendingHotkey")
      .and_then(|v| v.as_u64())
      .map(|n| n as u8);

    Some(DesktopProbeResult {
      is_auth,
      logged_out,
      typed,
      email,
      avatar,
      quota_text,
      quota_pct,
      quota_detail,
      pending_hotkey,
    })
  }

  fn is_view_instance_valid(locked: &InnerState, key: &AccountKey, view_instance_id: u64) -> bool {
    #[cfg(desktop)]
    {
      locked
        .warm_pool
        .get(key)
        .map(|e| e.view_instance_id == view_instance_id)
        .unwrap_or(false)
    }
    #[cfg(not(desktop))]
    {
      locked
        .active_slot
        .as_ref()
        .map(|s| s.account_key == *key && s.view_instance_id == view_instance_id)
        .unwrap_or(false)
    }
  }

  async fn apply_logged_out_state(
    &self,
    app: &AppHandle,
    key: AccountKey,
    view_instance_id: u64,
  ) {
    let snap_opt = {
      let mut locked = self.inner.lock().await;
      if !Self::is_view_instance_valid(&locked, &key, view_instance_id) {
        return;
      }

      let Some(target) = locked
        .store
        .accounts
        .iter_mut()
        .find(|a| a.provider_id == key.provider_id && a.uuid == key.uuid)
      else {
        return;
      };

      if target.auth_state == AuthState::LoggedOut
        && target.quota_detail.is_none()
        && target.quota_text.is_none()
        && target.quota_percent.is_none()
      {
        return;
      }

      target.auth_state = AuthState::LoggedOut;
      target.quota_detail = None;
      target.quota_text = None;
      target.quota_percent = None;

      if save_store_atomic(&locked.store_path, &locked.store).is_err() {
        return;
      }

      Some(Self::snapshot_from_locked(&locked))
    };

    if let Some(snap) = snap_opt {
      self.emit_snapshot(app, &snap);
    }
  }

  #[allow(clippy::too_many_arguments)]
  async fn apply_detected_metadata(
    &self,
    app: &AppHandle,
    key: AccountKey,
    view_instance_id: u64,
    identity_opt: Option<String>,
    avatar_opt: Option<String>,
    quota_text_opt: Option<String>,
    quota_pct_opt: Option<u8>,
    quota_detail_opt: Option<QuotaDetail>,
  ) {
    let snap_opt = {
      let mut locked = self.inner.lock().await;
      if !Self::is_view_instance_valid(&locked, &key, view_instance_id) {
        return;
      }

      let Some(target) = locked
        .store
        .accounts
        .iter_mut()
        .find(|a| a.provider_id == key.provider_id && a.uuid == key.uuid)
      else {
        return;
      };

      let mut changed = false;
      let mut alias_updated = None;

      if target.auth_state != AuthState::Authenticated {
        target.auth_state = AuthState::Authenticated;
        changed = true;
      }

      if target.auto_alias {
        if let Some(raw_id) = identity_opt {
          let cleaned = raw_id.trim();
          if !cleaned.is_empty() && cleaned.len() <= 80 {
            target.alias = cleaned.to_string();
            target.auto_alias = false;
            alias_updated = Some(cleaned.to_string());
            changed = true;
          }
        }
      }

      if let Some(av) = avatar_opt {
        if av.starts_with("data:image/") && av.len() < 80_000 && target.avatar_url.as_ref() != Some(&av) {
          target.avatar_url = Some(av);
          changed = true;
        }
      }

      if let Some(detail) = quota_detail_opt {
        if target.quota_detail.as_ref() != Some(&detail) {
          target.quota_detail = Some(detail);
          changed = true;
        }
      }

      if let Some(qt) = quota_text_opt {
        let cleaned_qt = qt.trim();
        if !cleaned_qt.is_empty() && cleaned_qt.len() <= 60 {
          let is_placeholder = cleaned_qt == "额度充足" || cleaned_qt == "正在同步额度…";
          let should_overwrite = match (&target.quota_text, is_placeholder) {
            (Some(existing), true) if existing != "额度充足" && existing != "正在同步额度…" => false,
            (Some(existing), _) if existing == cleaned_qt && target.quota_percent == quota_pct_opt => false,
            _ => true,
          };
          if should_overwrite {
            target.quota_text = Some(cleaned_qt.to_string());
            target.quota_percent = quota_pct_opt;
            changed = true;
          }
        }
      }

      if !changed {
        return;
      }

      if save_store_atomic(&locked.store_path, &locked.store).is_err() {
        return;
      }

      if let Some(new_alias) = alias_updated {
        let is_fg = locked
          .active_slot
          .as_ref()
          .map(|s| s.committed && s.account_key == key)
          .unwrap_or(false);
        if is_fg {
          let title = format!(
            "{} · {}",
            get_provider(key.provider_id).info.short_name,
            new_alias
          );
          #[cfg(desktop)]
          if let Some(main_win) = app.get_window("main") {
            let _ = main_win.set_title(&title);
          }
          #[cfg(target_os = "android")]
          if let Some(slot) = &locked.active_slot {
            let _ = crate::platform::android::update_account_title(app, slot.switch_seq, &title).await;
          }
        }
      }

      Some(Self::snapshot_from_locked(&locked))
    };

    if let Some(snap) = snap_opt {
      self.emit_snapshot(app, &snap);
    }
  }
}

#[cfg(desktop)]
struct DesktopProbeResult {
  is_auth: bool,
  logged_out: bool,
  typed: String,
  email: String,
  avatar: String,
  quota_text: String,
  quota_pct: Option<u8>,
  quota_detail: Option<QuotaDetail>,
  pending_hotkey: Option<u8>,
}
