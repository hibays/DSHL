// DSHL startup page — polls the Rust backend for state and renders it.
// The bound backend functions (get_state, retry, open_config, exit_app) are
// injected by webui.js as globals returning Promises.

"use strict";

const $ = (id) => document.getElementById(id);
const STATUS_TEXT = { pending: "等待", running: "进行中", done: "完成", error: "失败", skipped: "跳过" };
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
    ce.textContent = `配置文件加载失败，已使用默认配置：\n${state.config_error}`;
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
        rows.push([`mirrors.${k}`, v || "(空)"]);
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

function renderStatus(state) {
  const badge = $("status-badge");
  if (state.url) {
    badge.textContent = "已启动 → 跳转…";
    badge.style.color = "var(--ok)";
  } else if (state.error) {
    badge.textContent = "启动失败";
    badge.style.color = "var(--err)";
  } else {
    badge.textContent = "启动中…";
    badge.style.color = "var(--accent)";
  }
}

function render(state) {
  renderSteps(state.steps || []);
  renderConfig(state);
  renderLog(state.logs || []);
  renderError(state);
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
poll();
