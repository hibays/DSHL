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
    const dot = li.querySelector(".dot");
    const wanted = `dot ${DOT_CLASS[s.status] || ""}`.trim();
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
  const pathEl = $("config-path");
  const pathWanted = state.config_path ? `(${state.config_path})` : "";
  if (pathEl.textContent !== pathWanted) pathEl.textContent = pathWanted;
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
    // Unparseable config: show the raw text as-is. textContent comparison
    // covers both single-textNode and element-children layouts (k/v rows),
    // so the node-type check is no longer needed.
    const raw = state.config_json || "";
    if (view.textContent !== raw) view.textContent = raw;
    return;
  }

  // Incremental rebuild with a single key→{k,v} map. The existing rows are
  // indexed by their key text, so:
  //   * keys whose order changed are re-shuffled in the DOM,
  //   * new keys are inserted in the right position,
  //   * vanished keys are removed,
  //   * unchanged keys keep their existing DOM nodes (no flicker).
  const kEls = view.querySelectorAll(":scope > .k");
  const vEls = view.querySelectorAll(":scope > .v");
  // If the container holds ANY other children — including the single text
  // node a parseError pass leaves behind (textContent = raw) — wipe and
  // start clean. childElementCount cannot see text nodes; childNodes can.
  if (kEls.length !== vEls.length || kEls.length * 2 !== view.childNodes.length) {
    view.textContent = "";
    kEls.length = vEls.length = 0;
  }
  const existing = new Map();  // key → { k: HTMLElement, v: HTMLElement }
  for (let i = 0; i < kEls.length; i++) {
    existing.set(kEls[i].textContent, { k: kEls[i], v: vEls[i] });
  }

  const wantedKeys = new Set();
  for (let i = 0; i < rows.length; i++) {
    const [k, rawVal] = rows[i];
    const v = rawVal == null ? "" : String(rawVal);
    wantedKeys.add(k);
    let pair = existing.get(k);
    if (!pair) {
      pair = {
        k: Object.assign(document.createElement("div"), { className: "k", textContent: k }),
        v: Object.assign(document.createElement("div"), { className: "v", textContent: v }),
      };
      existing.set(k, pair);
    } else {
      if (pair.k.textContent !== k) pair.k.textContent = k;
      if (pair.v.textContent !== v) pair.v.textContent = v;
    }
    // Append/move the pair to the end so DOM order matches rows order.
    view.appendChild(pair.k);
    view.appendChild(pair.v);
  }

  // Remove orphan pairs whose key wasn't in rows.
  for (const [key, pair] of existing) {
    if (!wantedKeys.has(key)) {
      pair.k.remove();
      pair.v.remove();
    }
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
  const firstRender = logRendered === 0;
  const backendTruncated = lines.length < logRendered;
  if (firstRender || backendTruncated) {
    // First paint OR the backend truncated the array: full rebuild is the
    // only honest way to reflect the new contents.
    el.textContent = lines.join("\n");
  } else if (lines.length > logRendered) {
    // Append delta as a text node so the browser doesn't destroy the existing
    // text (which would clear any user selection inside the log panel and
    // restart the blinking cursor animation on every poll).
    if (logRendered > 0) {
      el.appendChild(document.createTextNode("\n"));
    }
    el.appendChild(document.createTextNode(lines.slice(logRendered).join("\n")));
  }
  // else: lines.length === logRendered — nothing to do, leave DOM alone.
  logRendered = lines.length;
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
  // data-status drives the badge colour via CSS (see styles.css .status[]),
  // so theme switches never fight inline-style specificity.
  if (state.crash && state.crash.countdown > 0) {
    badge.textContent = tr("page.badge.crash");
    badge.dataset.status = "crash";
  } else if (state.url) {
    badge.textContent = tr("page.badge.started");
    badge.dataset.status = "started";
  } else if (state.error) {
    badge.textContent = tr("page.badge.failed");
    badge.dataset.status = "failed";
  } else {
    badge.textContent = tr("page.badge.starting");
    badge.dataset.status = "starting";
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
