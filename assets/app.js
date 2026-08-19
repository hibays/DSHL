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
  // Incremental render: rows are updated in place and only touched when
  // their status/text actually changed. Rebuilding the list on every poll
  // would destroy and recreate each .dot every 250ms, restarting the running
  // step's CSS animation every time — the core would flicker instead of
  // swelling smoothly.
  while (ol.children.length > steps.length) ol.lastElementChild.remove();
  for (let i = 0; i < steps.length; i++) {
    let li = ol.children[i];
    if (!li) {
      li = document.createElement("li");
      li.innerHTML =
        `<span class="dot"></span>` +
        `<div class="step-body">` +
        `<div class="step-title"></div>` +
        `<div class="step-msg"></div>` +
        `</div>`;
      ol.appendChild(li);
    }
    const s = steps[i];
    const cls = DOT_CLASS[s.status] || "";
    const dot = li.querySelector(".dot");
    const wanted = cls ? `dot ${cls}` : "dot";
    if (dot.className !== wanted) dot.className = wanted;
    const title = li.querySelector(".step-title");
    const msg = li.querySelector(".step-msg");
    const text = s.title;
    if (title.textContent !== text) title.textContent = text;
    const sub = s.message || "";
    if (msg.textContent !== sub) msg.textContent = sub;
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
  let rows = [];
  let parseError = false;
  try {
    const cfg = JSON.parse(state.config_json || "{}");
    rows = [
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
  } catch (e) {
    parseError = true;
  }
  if (parseError) {
    const raw = state.config_json || "";
    // Unparseable config: show the raw text as-is (full replace is fine here —
    // this is a stable error state, not a per-poll rebuild).
    const first = view.firstChild;
    if (!(first && first.nodeType === 3 && first.textContent === raw)) {
      view.textContent = raw;
    }
    return;
  }
  // Incremental render: keep the existing k/v rows, updating only the text
  // that changed and adding/removing rows to match the count. The rows are
  // alternating div.k / div.v children, so row i lives at children[2i], [2i+1].
  const needed = rows.length * 2;
  while (view.childNodes.length > needed) view.lastChild.remove();
  for (let i = 0; i < rows.length; i++) {
    const k = rows[i][0];
    const v = rows[i][1] === undefined || rows[i][1] === null ? "" : String(rows[i][1]);
    let kEl = view.childNodes[i * 2];
    if (!kEl) {
      kEl = document.createElement("div");
      kEl.className = "k";
      view.appendChild(kEl);
    }
    if (kEl.textContent !== k) kEl.textContent = k;
    let vEl = view.childNodes[i * 2 + 1];
    if (!vEl) {
      vEl = document.createElement("div");
      vEl.className = "v";
      view.appendChild(vEl);
    }
    if (vEl.textContent !== v) vEl.textContent = v;
  }
}

// Log follows the user: pinned to the bottom while new entries arrive, and
// the moment the user scrolls up it stays put (no yank). Scrolling back to
// the bottom re-pins. (The page has no scroll listener besides these two.)
let logPinned = true;
// Lines already in the log panel — appended incrementally so a stable log
// isn't rebuilt (and selection isn't lost) on every 250ms poll.
let logRendered = 0;

function renderLog(lines) {
  const el = $("log");
  if (logRendered > 0 && lines.length >= logRendered) {
    if (lines.length > logRendered) {
      el.textContent += "\n" + lines.slice(logRendered).join("\n");
      logRendered = lines.length;
    }
    // unchanged — leave the DOM alone
  } else {
    // First render, or the backend truncated the log — rebuild from scratch.
    el.textContent = lines.join("\n");
    logRendered = lines.length;
  }
  if (logPinned) el.scrollTop = el.scrollHeight;
}

function syncFooterPad() {
  // Reserve exactly the fixed action bar's height (it wraps on narrow
  // screens), plus breathing room — no oversized placeholder that forces a
  // page scrollbar on short landscape viewports.
  const f = document.querySelector("footer");
  document.documentElement.style.setProperty("--footer-h", `${f.offsetHeight}px`);
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
  // body.running drives the breathing LED in the status stamp (and nothing
  // else): it is set exactly while the launch is in progress.
  const starting =
    !(state.crash && state.crash.countdown > 0) && !state.url && !state.error;
  document.body.classList.toggle("running", starting);
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
    badge.style.color = "var(--accent-soft)";
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
  // Footer width/height can change as buttons appear or wrap — re-measure
  // the room it needs before the browser paints this frame.
  syncFooterPad();
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

const logEl = $("log");
logEl.addEventListener("scroll", () => {
  logPinned = logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight < 16;
});
window.addEventListener("resize", syncFooterPad);
