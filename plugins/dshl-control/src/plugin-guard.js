// @dshl/control — plugin disable guard + crash rollback.
// Independent design (not aligned with deepseek-harness-desktop protocol).
//
// Persists two files under `$DSH_HOME/.dshl/` (or ~/.dsh/.dshl/ fallback):
//   - disabled.json      : disabled plugins map (pkg -> {reason, disabledAt})
//   - launch-state.json  : crash tracking (healthy / last healthy bundles snapshot / consecutive crashes)
//
// Rollback rule (kept intentionally very simple):
//   - On apply (beginStartup) we write startedAt + healthy=false.
//   - If the renderer calls markHealthy({bundles}) within WINDOW_MS (default 30s),
//     we commit healthy=true, save bundles snapshot as lastHealthyBundles, reset crash counter.
//   - On next startup, if (healthy was false AND startedAt is older than GRACE_MS=10s),
//     we treat previous run as "launched and then crashed without reporting healthy".
//     consecutiveCrashes++ and any bundle present in the "current bundle list" but NOT in
//     lastHealthyBundles is flagged as "suspicious new / recently changed".
//     Once consecutiveCrashes >= AUTO_DISABLE_THRESHOLD (3) we write those suspicious bundles
//     into disabled.json with reason="crash-3x".
//
// ENFORCEMENT CAVEAT (do not paper over this): nothing in the dsh plugin
// loader reads disabled.json today — the bundle patch mechanism can only
// INSERT a plugin, not filter others out. The disable list is persisted and
// exposed (dshlPluginGuard / desktopPlugins services, the guard HTTP routes
// and the overlay UI); it is bookkeeping + visibility, NOT a load-time
// block. Every user-facing claim must say "recorded", never "won't load on
// next boot", until a consumer in the loader actually exists.

import { createRequire } from 'node:module';
import { createHash, randomBytes } from 'node:crypto';
import { homedir } from 'node:os';
import { resolve, dirname } from 'node:path';
import fs from 'node:fs';

const require = createRequire(import.meta.url);

const DSHL_DIRNAME = '.dshl';
const DISABLED_FILE = 'disabled.json';
const STATE_FILE = 'launch-state.json';

const WINDOW_MS = 30_000;      // renderer must call markHealthy within 30s after launch
const GRACE_MS = 10_000;       // 10s grace: "startedAt recent but no healthy yet" is not a crash
const AUTO_DISABLE_THRESHOLD = 3;
const OUR_PKG = '@dshl/control';  // never allow disabling the guard itself

function sha1Hex(s) { return createHash('sha1').update(s).digest('hex'); }
function utcIso() { return new Date().toISOString(); }
function mkdirp(p) { fs.mkdirSync(p, { recursive: true, mode: 0o700 }); }

// Atomic write (to temp in same dir -> rename). dir MUST exist.
function atomicWriteJson(path, value) {
  const tmp = `${path}.${process.pid}-${randomBytes(4).toString('hex')}.tmp`;
  try {
    fs.writeFileSync(tmp, JSON.stringify(value, null, 2) + '\n', { mode: 0o600, flag: 'wx' });
    fs.renameSync(tmp, path);
  } catch (e) {
    try { fs.unlinkSync(tmp); } catch { /* ignore */ }
    throw e;
  }
}

function readJson(path, fallback) {
  try {
    const raw = fs.readFileSync(path, 'utf8');
    return JSON.parse(raw);
  } catch (e) {
    if (e && e.code === 'ENOENT') return fallback;
    return fallback;
  }
}

function resolveRootDir() {
  const home = process.env.DSH_HOME || process.env.DSHL_CACHE || resolve(homedir(), '.dsh');
  const root = resolve(home, DSHL_DIRNAME);
  mkdirp(root);
  return root;
}

// --- disabled.json schema v1 ------------------------------------------------
// { version: 1, disabled: Record<pkg, {reason: string, disabledAt: string (ISO)}> }
function emptyDisabled() { return { version: 1, disabled: {} }; }

function loadDisabled(root) {
  const raw = readJson(resolve(root, DISABLED_FILE), null);
  if (!raw || typeof raw !== 'object' || raw.version !== 1 || !raw.disabled) {
    return emptyDisabled();
  }
  return raw;
}

function saveDisabled(root, data) { atomicWriteJson(resolve(root, DISABLED_FILE), data); }

// --- launch-state.json schema v1 -------------------------------------------
// {
//   version: 1,
//   startedAt: ISO | null,  // timestamp we wrote at apply (beginStartup)
//   healthy: boolean,        // whether current startedAt run has been marked healthy
//   lastHealthyAt: ISO | null,
//   lastHealthyBundles: string[] | null,  // sorted deduped package list when last healthy
//   consecutiveCrashes: number,
//   rollback: {
//     enabled: boolean,           // true if this launch will be a "rollback" launch
//     crashedSnapshotBundles: string[] | null, // bundles that were active during crash
//     suspicious: string[]        // bundles present now but missing in lastHealthyBundles
//   }
// }
function emptyState() {
  return {
    version: 1,
    healthy: false,
    gracefulExit: false,
    lastHealthyAt: null,
    lastHealthyBundles: null,
    consecutiveCrashes: 0,
    rollback: { enabled: false, crashedSnapshotBundles: null, suspicious: [] },
  };
}

function loadState(root) {
  const raw = readJson(resolve(root, STATE_FILE), null);
  if (!raw || typeof raw !== 'object' || raw.version !== 1) return emptyState();
  const s = emptyState();
  for (const k of ['startedAt','healthy','lastHealthyAt','lastHealthyBundles','consecutiveCrashes']) if (k in raw) s[k] = raw[k];
  if (raw.rollback && typeof raw.rollback === 'object') Object.assign(s.rollback, raw.rollback);
  if (!Array.isArray(s.lastHealthyBundles)) s.lastHealthyBundles = null;
  for (const k of ['startedAt','healthy','gracefulExit','lastHealthyAt','lastHealthyBundles','consecutiveCrashes']) if (k in raw) s[k] = raw[k];
  return s;
}

function saveState(root, state) { atomicWriteJson(resolve(root, STATE_FILE), state); }

function normBundles(list) {
  const s = new Set();
  for (const b of (Array.isArray(list) ? list : [])) {
    if (typeof b === 'string' && b.length) s.add(b);
  }
  return [...s].sort();
}

// --- Guard API --------------------------------------------------------------

export class PluginGuard {
  constructor({ dshHome = null, currentBundleList = null } = {}) {
    this.root = dshHome ? resolve(dshHome, DSHL_DIRNAME) : resolveRootDir();
    mkdirp(this.root);
    this.disabled = loadDisabled(this.root);
    this.state = loadState(this.root);
    this._healthyTimer = null;
    this._startBundleSnapshot = normBundles(currentBundleList);
  }

  // ---------- low-level disk helpers ----------
  disabledSet() { return new Set(Object.keys(this.disabled.disabled || {})); }
  isDisabled(pkg) { return pkg === OUR_PKG ? false : !!this.disabled.disabled?.[pkg]; }
  disabledPackageNames() { return Object.keys(this.disabled.disabled || {}).sort(); }
  list({ bundles = null } = {}) {
    const disabled = this.disabledSet();
    const list = (Array.isArray(bundles) ? bundles : this._startBundleSnapshot).map((pkg) => ({
      packageName: pkg,
      id: 'b_' + sha1Hex(pkg).slice(0, 16),
      status: pkg === OUR_PKG ? 'protected' : disabled.has(pkg) ? 'disabled' : 'active',
      mutable: pkg !== OUR_PKG,
      disabledReason: this.disabled.disabled?.[pkg]?.reason ?? null,
      disabledAt: this.disabled.disabled?.[pkg]?.disabledAt ?? null,
    }));
    // Append any disabled entry that wasn't in bundles list
    for (const pkg of this.disabledPackageNames()) {
      if (list.find(e => e.packageName === pkg)) continue;
      list.push({
        packageName: pkg,
        id: 'b_' + sha1Hex(pkg).slice(0, 16),
        status: 'disabled',
        mutable: pkg !== OUR_PKG,
        disabledReason: this.disabled.disabled[pkg]?.reason ?? null,
        disabledAt: this.disabled.disabled[pkg]?.disabledAt ?? null,
      });
    }
    return list;
  }

  // ---------- disable / enable ----------
  disable(pkg, { reason = 'manual' } = {}) {
    if (!pkg || typeof pkg !== 'string') throw new Error('packageName required');
    if (pkg === OUR_PKG) throw new Error('cannot disable the guard plugin itself');
    const now = utcIso();
    this.disabled.disabled[pkg] = { reason: String(reason), disabledAt: now };
    saveDisabled(this.root, this.disabled);
    return { packageName: pkg, disabled: true, needRestart: true, at: now };
  }

  enable(pkg) {
    if (!this.disabled.disabled?.[pkg]) {
      return { packageName: pkg, disabled: false, needRestart: true, hadEntry: false };
    }
    delete this.disabled.disabled[pkg];
    saveDisabled(this.root, this.disabled);
    return { packageName: pkg, disabled: false, needRestart: true, hadEntry: true };
  }

  // ---------- lifecycle: beginStartup / markHealthy / markFailed ----------
  beginStartup({ currentBundles = null } = {}) {
    const now = utcIso();
    const bundles = normBundles(currentBundles);
    if (bundles.length) this._startBundleSnapshot = bundles;

    // Pre-startup rollback analysis: previous run may have crashed without markHealthy.
    // A run that ended through the plugin's dispose hook is a GRACEFUL exit
    // (see markShutdown) — never counted as a crash, no matter whether the
    // renderer got to call markHealthy (user may close before the UI loads).
    const prevStarted = this.state.startedAt ? Date.parse(this.state.startedAt) : NaN;
    const prevHealthy = !!this.state.healthy;
    const prevGraceful = !!this.state.gracefulExit;
    const prevAgeMs = Number.isFinite(prevStarted) ? (Date.now() - prevStarted) : 0;
    let shouldCountCrash = !prevHealthy && !prevGraceful && Number.isFinite(prevStarted) && prevAgeMs >= GRACE_MS;
    this.state.rollback = { enabled: false, crashedSnapshotBundles: null, suspicious: [] };

    if (shouldCountCrash) {
      this.state.consecutiveCrashes = (this.state.consecutiveCrashes || 0) + 1;
      // snapshot = what bundles were active last time (approx = start snapshot now, but we
      // don't persist the started bundles separately; approximate with lastHealthyBundles).
      const current = this._startBundleSnapshot;
      const lastOk = Array.isArray(this.state.lastHealthyBundles) ? new Set(this.state.lastHealthyBundles) : null;
      const suspicious = lastOk
        ? current.filter(b => b !== OUR_PKG && !lastOk.has(b))
        : [];
      this.state.rollback = {
        enabled: this.state.consecutiveCrashes >= AUTO_DISABLE_THRESHOLD && suspicious.length > 0,
        crashedSnapshotBundles: Array.isArray(this.state.lastHealthyBundles)
          ? [...this.state.lastHealthyBundles]
          : null,
        suspicious,
      };
      // auto-disable if threshold met
      if (this.state.rollback.enabled) {
        const at = utcIso();
        for (const b of suspicious) {
          if (b === OUR_PKG) continue;
          this.disabled.disabled[b] = { reason: `crash-${this.state.consecutiveCrashes}x`, disabledAt: at };
        }
        saveDisabled(this.root, this.disabled);
      }
    } else if (prevHealthy) {
      // previous run finished healthy (or was healthy before graceful shutdown) → keep counter as-is but don't grow
      this.state.consecutiveCrashes = this.state.consecutiveCrashes || 0;
    }
    // Prepare for THIS run
    this.state.startedAt = now;
    this.state.healthy = false;
    this.state.gracefulExit = false; // fresh run: the marker applies to the PREVIOUS one only
    saveState(this.root, this.state);

    // Safety deadline: mark failed (non-committal; does not disable alone) if no markHealthy within WINDOW_MS
    if (this._healthyTimer) clearTimeout(this._healthyTimer);
    this._healthyTimer = setTimeout(() => {
      if (!this.state.healthy && this.state.startedAt === now) {
        // Soft-fail: renderer never reported in. Do not auto-disable based on timeout alone
        // (user could just close the browser window). We only count crash on next boot.
      }
    }, WINDOW_MS + 1000);
    if (this._healthyTimer.unref) this._healthyTimer.unref();

    return {
      consecutiveCrashes: this.state.consecutiveCrashes,
      rollback: this.state.rollback,
      autoDisabledThisRound: this.state.rollback.enabled ? [...this.state.rollback.suspicious] : [],
    };
  }

  markHealthy({ bundles = null } = {}) {
    const snap = normBundles(bundles);
    this.state.healthy = true;
    this.state.lastHealthyAt = utcIso();
    if (snap.length) this.state.lastHealthyBundles = snap;
    this.state.consecutiveCrashes = 0;
    this.state.rollback = { enabled: false, crashedSnapshotBundles: null, suspicious: [] };
    saveState(this.root, this.state);
    if (this._healthyTimer) { clearTimeout(this._healthyTimer); this._healthyTimer = null; }
    return { healthy: true, at: this.state.lastHealthyAt, bundles: this.state.lastHealthyBundles || [] };
  }

  // Called from the plugin dispose hook: this run ended through normal
  // teardown (harness shutdown), NOT a crash. The next beginStartup will not
  // count it even when the renderer never reached markHealthy.
  markShutdown() {
    this.state.gracefulExit = true;
    saveState(this.root, this.state);
    if (this._healthyTimer) { clearTimeout(this._healthyTimer); this._healthyTimer = null; }
    return { gracefulExit: true, at: utcIso() };
  }

  markFailed({ report = null } = {}) {
    // explicit failure from UI/renderer (e.g. window.onerror bucketed / loader init threw).
    // Do NOT auto-disable here; leave detection until next beginStartup() (avoids false positives).
    this.state.healthy = false;
    saveState(this.root, this.state);
    if (this._healthyTimer) { clearTimeout(this._healthyTimer); this._healthyTimer = null; }
    return { failed: true, at: utcIso(), report: typeof report === 'string' ? report.slice(0, 4000) : null };
  }

  // ---------- diagnostics ----------
  nextStartupRollbackInfo() {
    return {
      consecutiveCrashes: this.state.consecutiveCrashes || 0,
      autoDisableThreshold: AUTO_DISABLE_THRESHOLD,
      healthy: !!this.state.healthy,
      startedAt: this.state.startedAt ?? null,
      lastHealthyAt: this.state.lastHealthyAt ?? null,
      lastHealthyBundles: this.state.lastHealthyBundles ?? [],
      rollback: this.state.rollback,
    };
  }

  // File paths (for debugging)
  paths() {
    return { root: this.root, disabled: resolve(this.root, DISABLED_FILE), state: resolve(this.root, STATE_FILE) };
  }
}

export default PluginGuard;
