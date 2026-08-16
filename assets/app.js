// DSHL startup page — polls the Rust backend for state and renders it.
// The bound backend functions (get_state, retry, open_config, exit_app) are
// injected by webui.js as globals returning Promises.

"use strict";

const $ = (id) => document.getElementById(id);

// i18n: /i18n.js (served by the backend vfs, loaded before this file) exposes
// window.DSHL_LOCALE and window.DSHL_I18N. tr() looks up a key and replaces
// %{var} placeholders; unknown keys fall back to the key itself.
function tr(key, vars) {
  let s = (window.DSHL_I18N && window.DSHL_I18N[key]) || key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      s = s.split(`%{${k}}`).join(String(v));
    }
  }
  return s;
}

// Apply translations to every element marked with data-i18n (set once at
// startup; static markup keeps its translated text for the whole session).
function applyDataI18n() {
  document.documentElement.lang = window.DSHL_LOCALE || "zh-CN";
  for (const el of document.querySelectorAll("[data-i18n]")) {
    const key = el.getAttribute("data-i18n");
    const v = tr(key);
    if (el.id === "status-badge") {
      // The badge's running text is redrawn by renderStatus() on every poll,
      // so only the initial value matters here.
      el.textContent = v;
    } else if (el.tagName === "TITLE") {
      document.title = v;
    } else {
      el.textContent = v;
    }
  }
}

const STATUS_TEXT = {
  pending: tr("page.status.pending"),
  running: tr("page.status.running"),
  done: tr("page.status.done"),
  error: tr("page.status.error"),
  skipped: tr("page.status.skipped"),
};
const DOT_CLASS = { pending: "", running: "running", done: "done", error: "error", skipped: "skipped" };

// The dsh web URL as last reported by the backend (set once dsh is up).
// The launcher page is served by webui's local server, so jumping back to
// the dsh deploy page is a plain same-window navigation — no backend call.
// sessionStorage survives a full reload (non-bfcache back navigation), so
// the button shows immediately even before the first poll succeeds.
let dshUrl = sessionStorage.getItem("dshl:dsh-url") || "";

function renderSteps(steps) {
  const ol = $("steps");
  ol.innerHTML = "";
  for (const s of steps) {
    const li = document.createElement("li");
    li.innerHTML =
      `<span class="dot ${DOT_CLASS[s.status] || ""}"></span>` +
      `<div class="step-body">` +
      `<div class="step-title">${escapeHtml(s.title)}</div>` +
      `<div class="step-msg">${escapeHtml(s.message || "")}</div>` +
      `</div>`;
    ol.appendChild(li);
  }
}

function renderConfig(state) {
  $("config-path").textContent = state.config_path ? `(${state.config_path})` : "";
  const ce = $("config-error");
  if (state.config_error) {
    ce.hidden = false;
    ce.textContent = tr("page.config_error_prefix", { err: state.config_error });
  } else {
    ce.hidden = true;
  }
  const view = $("config-view");
  view.innerHTML = "";
  try {
    const cfg = JSON.parse(state.config_json || "{}");
    const rows = [
      ["auto-mirror", cfg.auto_mirror],
      ["dsh.mode", cfg.dsh && cfg.dsh.mode],
      ["dsh.pm", cfg.dsh && cfg.dsh.pm],
      ["dsh.exector", cfg.dsh && cfg.dsh.exector],
      ["dsh.version", cfg.dsh && cfg.dsh.version],
      ["dsh.flags", cfg.dsh && cfg.dsh.flags],
    ];
    if (cfg.mirrors) {
      for (const [k, v] of Object.entries(cfg.mirrors)) {
        rows.push([`mirrors.${k}`, v || tr("page.empty_value")]);
      }
    }
    for (const [k, v] of rows) {
      const kEl = document.createElement("div");
      kEl.className = "k";
      kEl.textContent = k;
      const vEl = document.createElement("div");
      vEl.className = "v";
      vEl.textContent = v === undefined || v === null ? "" : String(v);
      view.appendChild(kEl);
      view.appendChild(vEl);
    }
  } catch (e) {
    view.textContent = state.config_json || "";
  }
}

function renderLog(lines) {
  const el = $("log");
  el.textContent = lines.join("\n");
  el.scrollTop = el.scrollHeight;
}

function renderError(state) {
  const el = $("error");
  if (state.error) {
    el.hidden = false;
    el.textContent = state.error;
  } else {
    el.hidden = true;
  }
}

// Crash-recovery banner: dsh exited unexpectedly and an auto-restart is
// counting down (立即重启 / 取消 call the bound restart_now / cancel_restart
// backend functions; the countdown itself runs on the Rust side).
function renderCrash(state) {
  const el = $("crash");
  const c = state.crash;
  if (c && c.countdown > 0) {
    el.hidden = false;
    $("crash-msg").textContent = tr("page.crash.message", {
      code: c.code,
      countdown: c.countdown,
    });
  } else {
    el.hidden = true;
  }
}

function renderStatus(state) {
  const badge = $("status-badge");
  // During crash recovery there is no running dsh — don't claim otherwise.
  if (state.crash && state.crash.countdown > 0) {
    badge.textContent = tr("page.badge.crash");
    badge.style.color = "var(--err)";
  } else if (state.url) {
    badge.textContent = tr("page.badge.started");
    badge.style.color = "var(--ok)";
  } else if (state.error) {
    badge.textContent = tr("page.badge.failed");
    badge.style.color = "var(--err)";
  } else {
    badge.textContent = tr("page.badge.starting");
    badge.style.color = "var(--accent)";
  }
}

function render(state) {
  renderSteps(state.steps || []);
  renderConfig(state);
  renderLog(state.logs || []);
  renderError(state);
  renderCrash(state);
  renderStatus(state);
  // Force-kill confirmation button: visible only while a stale dsh process
  // is waiting for the user to decide. Clicking it calls the bound
  // force_kill_stale() backend function — the click IS the confirmation.
  $("btn-force-kill").hidden = !state.stale_pid;
  // Manual jump back to the dsh deploy page: available once dsh has
  // reported its URL (e.g. after the user navigated back to this page).
  dshUrl = state.url || "";
  if (dshUrl) {
    sessionStorage.setItem("dshl:dsh-url", dshUrl);
  } else {
    sessionStorage.removeItem("dshl:dsh-url");
  }
  // Hide the jump button while the crashed dsh is recovering (its URL is
  // gone anyway — `crash::begin` clears it).
  $("btn-open-dsh").hidden = !dshUrl;
}

// Jump to the dsh deploy page. The backend navigates there automatically on
// launch; this is for when the user navigated back to the launcher page.
function open_dsh() {
  if (dshUrl) {
    location.assign(dshUrl);
  }
}

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

async function poll() {
  try {
    const res = await get_state();
    render(JSON.parse(res));
  } catch (e) {
    /* backend not ready yet */
  }
}

setInterval(poll, 250);
applyDataI18n();
poll();
