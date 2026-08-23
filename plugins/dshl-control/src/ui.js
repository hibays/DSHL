// @dshl/control web widget: floating launcher action bar + guard overlay,
// injected into the dsh web UI by the bundle's tapIndex.
//
// - Bar buttons: Terminal (opens embedded xterm.js panel), Console,
//   Restart, Shutdown, Guard (opens overlay).
// - Guard overlay: shows crash count / rollback state / disabled list,
//   and exposes disable/enable + manual markHealthy/markFailed testing.
// - Terminal panel: xterm.js rendered in-browser, connected to dshl-core's
//   standalone PTY WebSocket server (see dshl_core::pty). xterm.js and the
//   fit addon are vendored (assets/xterm, served from this plugin at
//   /dshl-control/assets/xterm/*) so the panel works fully offline. PTY
//   resize flows over WS as a JSON control message (NOT as terminal escape
//   codes), avoiding the pitfalls documented in experience 1514774.
(function () {
  'use strict'
  if (window.__dshlControlUiLoaded) return
  window.__dshlControlUiLoaded = true

  // ---- Import map for the vendored xterm.js ---------------------------------
  // Served by this same plugin from /dshl-control/assets/xterm/* (files in
  // assets/xterm, see that directory's README). NEVER switch these to a CDN:
  // the launcher must work fully offline / in restricted networks, and the
  // historical jsdelivr URLs pointed at a version that never existed on npm.
  if (!document.querySelector('script[type="importmap"]')) {
    var im = document.createElement('script')
    im.type = 'importmap'
    im.textContent = JSON.stringify({
      imports: {
        '@xterm/xterm': '/dshl-control/assets/xterm/xterm.mjs',
        '@xterm/addon-fit': '/dshl-control/assets/xterm/addon-fit.mjs',
      },
    })
    document.head.appendChild(im)
  }
  // Ensure xterm.css is loaded via a plain <link> (CSS is not an importable
  // module specifier — an importmap entry for it would be inert decoration).
  if (!document.querySelector('link[data-dshl-xterm-css]')) {
    var lnk = document.createElement('link')
    lnk.rel = 'stylesheet'
    lnk.href = '/dshl-control/assets/xterm/xterm.css'
    lnk.setAttribute('data-dshl-xterm-css', '1')
    document.head.appendChild(lnk)
  }

  var STATE_URL = '/dshl-control/state'
  var GUARD_LIST = '/dshl-control/plugins/list'
  var GUARD_DISABLED = '/dshl-control/plugins/disabled'
  var GUARD_ROLLBACK = '/dshl-control/plugins/rollback'
  var GUARD_MARK_HEALTHY = '/dshl-control/plugins/mark-healthy'
  var GUARD_MARK_FAILED = '/dshl-control/plugins/mark-failed'
  var GUARD_ACTION = function (name, verb) { return '/dshl-control/plugins/' + encodeURIComponent(name) + '/' + verb }

  // ---- Locale helper ---------------------------------------------------------
  // The bar/overlay texts are user-facing; follow the browser language
  // (the dsh host page has no i18n plumbing to borrow).
  var IS_ZH = String((typeof navigator !== 'undefined' && navigator.language) || '').toLowerCase().indexOf('zh') === 0
  function T(zh, en) { return IS_ZH ? zh : en }

  // ---- Base styles ----------------------------------------------------------
  // DELIBERATE: this widget follows the dsh HOST page's own visual language
  // (rounded pills, soft shadows, the host's neutral/blue palette) so the
  // injected bar/overlay read as part of dsh, not as a foreign element — see
  // the upstream repos (deepseek-harness apps/web design tokens and
  // deepseek-harness-desktop's dsh-plugin-desktop). The launcher's own
  // DESIGN.md Swiss system governs only the startup page (assets/), NOT this
  // injected UI.
  var style = document.createElement('style')
  style.textContent =
    '#dshl-control-bar{position:fixed;right:16px;bottom:16px;z-index:2147483000;display:flex;gap:8px;flex-wrap:wrap;font-family:ui-sans-serif,system-ui,sans-serif;align-items:center;}' +
    '#dshl-control-bar button{appearance:none;border:1px solid rgba(127,127,127,.35);background:rgba(24,24,27,.92);color:#e4e4e7;border-radius:9999px;padding:6px 14px;font-size:13px;line-height:1;cursor:pointer;box-shadow:0 2px 8px rgba(0,0,0,.25);transition:background .15s ease;}' +
    '#dshl-control-bar button:hover{background:#27272a;}' +
    '#dshl-control-bar button:active{transform:translateY(1px);}' +
    '#dshl-control-bar .badge{display:inline-flex;align-items:center;justify-content:center;min-width:20px;height:20px;padding:0 6px;margin-left:6px;border-radius:9999px;background:#ef4444;color:#fff;font-size:11px;font-weight:600;line-height:1;}' +
    '#dshl-control-bar .badge.warn{background:#f59e0b;color:#111827;}' +
    '#dshl-control-bar .badge.ok{background:#10b981;color:#fff;}' +
    '#dshl-control-overlay{position:fixed;inset:0;z-index:2147483500;background:rgba(0,0,0,.5);display:none;font-family:ui-sans-serif,system-ui,sans-serif;}' +
    '#dshl-control-overlay.show{display:flex;align-items:center;justify-content:center;}' +
    '#dshl-control-panel{width:min(760px,92vw);max-height:86vh;overflow:auto;background:#0b0b0f;color:#e5e7eb;border-radius:12px;border:1px solid rgba(148,163,184,.18);box-shadow:0 18px 60px rgba(0,0,0,.6);padding:18px 20px;}' +
    '#dshl-terminal-overlay{position:fixed;inset:16px;z-index:2147483400;background:#0a0a0d;border-radius:14px;border:1px solid rgba(148,163,184,.22);box-shadow:0 22px 70px rgba(0,0,0,.75);display:none;flex-direction:column;overflow:hidden;}' +
    '#dshl-terminal-overlay.show{display:flex;}' +
    '#dshl-terminal-header{display:flex;align-items:center;justify-content:space-between;padding:10px 14px;border-bottom:1px solid rgba(148,163,184,.15);}' +
    '#dshl-terminal-header .title{font-family:ui-sans-serif,system-ui,sans-serif;font-size:13px;font-weight:600;color:#e5e7eb;display:flex;gap:8px;align-items:center;}' +
    '#dshl-terminal-header .dot{width:10px;height:10px;border-radius:9999px;background:#22c55e;flex:none;}' +
    '#dshl-terminal-header .dot.off{background:#ef4444;}' +
    '#dshl-terminal-header .meta{color:#94a3b8;font-size:11px;font-weight:400;}' +
    '#dshl-terminal-header .btn{background:#1f2937;color:#e5e7eb;border:1px solid rgba(148,163,184,.28);border-radius:8px;padding:4px 10px;font-size:12px;cursor:pointer;}' +
    '#dshl-terminal-header .btn.danger{background:#7f1d1d;border-color:rgba(248,113,113,.55);color:#fee2e2;}' +
    '#dshl-terminal-host{flex:1 1 auto;min-height:0;overflow:hidden;background:#030712;padding:10px 12px;}' +
    '#dshl-terminal-host .xterm{height:100%;width:100%;}' +
    '#dshl-terminal-host .xterm-viewport{overflow-y:auto;}' +
    '#dshl-terminal-footer{border-top:1px solid rgba(148,163,184,.14);padding:6px 14px;font-family:ui-mono,monospace;font-size:11px;color:#94a3b8;background:#0a0a0d;}' +
    '#dshl-control-panel header{display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;}' +
    '#dshl-control-panel h3{margin:0;font-size:15px;font-weight:600;color:#fafafa;}' +
    '#dshl-control-panel .row{display:flex;gap:10px;flex-wrap:wrap;align-items:center;}' +
    '#dshl-control-panel .close{background:transparent;border:1px solid rgba(148,163,184,.28);color:#cbd5e1;border-radius:8px;padding:4px 10px;cursor:pointer;font-size:12px;}' +
    '#dshl-control-panel .stat{flex:1 1 160px;background:rgba(30,41,59,.5);border:1px solid rgba(148,163,184,.12);border-radius:10px;padding:10px 12px;}' +
    '#dshl-control-panel .stat .k{font-size:11px;color:#94a3b8;letter-spacing:.02em;text-transform:uppercase;}' +
    '#dshl-control-panel .stat .v{font-size:18px;font-weight:600;margin-top:4px;color:#f8fafc;}' +
    '#dshl-control-panel section{margin-top:16px;}' +
    '#dshl-control-panel h4{margin:0 0 8px;font-size:13px;color:#cbd5e1;font-weight:600;}' +
    '#dshl-control-panel .pill{display:inline-block;padding:2px 8px;border-radius:9999px;font-size:11px;font-weight:600;line-height:1.6;margin-right:6px;}' +
    '#dshl-control-panel .pill.disabled{background:rgba(239,68,68,.15);color:#fca5a5;border:1px solid rgba(239,68,68,.3);}' +
    '#dshl-control-panel .pill.active{background:rgba(16,185,129,.15);color:#6ee7b7;border:1px solid rgba(16,185,129,.3);}' +
    '#dshl-control-panel .pill.protected{background:rgba(129,140,248,.15);color:#c7d2fe;border:1px solid rgba(129,140,248,.3);}' +
    '#dshl-control-panel ul.plugins{list-style:none;margin:0;padding:0;display:flex;flex-direction:column;gap:8px;}' +
    '#dshl-control-panel ul.plugins li{display:grid;grid-template-columns:1fr auto;gap:12px;align-items:center;padding:10px 12px;border:1px solid rgba(148,163,184,.14);border-radius:10px;background:rgba(15,23,42,.35);}' +
    '#dshl-control-panel ul.plugins .meta{font-size:11px;color:#94a3b8;margin-top:2px;}' +
    '#dshl-control-panel ul.plugins button{background:rgba(30,64,175,.35);color:#e0f2fe;border:1px solid rgba(59,130,246,.45);border-radius:8px;padding:5px 10px;font-size:12px;cursor:pointer;}' +
    '#dshl-control-panel ul.plugins button.secondary{background:rgba(51,65,85,.5);border-color:rgba(148,163,184,.3);color:#e2e8f0;}' +
    '#dshl-control-panel ul.plugins button:disabled{opacity:.5;cursor:not-allowed;}' +
    '#dshl-control-panel .actions{display:flex;gap:8px;flex-wrap:wrap;margin-top:6px;}' +
    '#dshl-control-panel .btn{background:#2563eb;color:#eff6ff;border:1px solid rgba(59,130,246,.6);border-radius:8px;padding:6px 12px;font-size:12px;cursor:pointer;font-weight:500;}' +
    '#dshl-control-panel .btn.alt{background:#0f766e;color:#ecfeff;border-color:rgba(20,184,166,.55);}' +
    '#dshl-control-panel .btn.danger{background:#b91c1c;color:#fee2e2;border-color:rgba(248,113,113,.55);}' +
    '#dshl-control-panel .muted{color:#94a3b8;font-size:11px;margin-top:4px;}' +
    '#dshl-control-panel .suspicious{color:#fca5a5;font-weight:600;}'
  document.head.appendChild(style)

  // ---- Floating bar ---------------------------------------------------------
  var bar = document.createElement('div')
  bar.id = 'dshl-control-bar'
  var badgeEl = null

  function button(label, onClick) {
    var el = document.createElement('button')
    el.type = 'button'
    el.textContent = label
    el.addEventListener('click', onClick)
    return el
  }

  function post(path, body, okText) {
    return fetch(path, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body || {}),
    }).then(function (r) {
      if (!r.ok) {
        return r.text().then(function (t) {
          var msg = t || ('HTTP ' + r.status)
          try {
            var j = JSON.parse(msg)
            if (j && j.error) {
              msg = j.code === 'booting'
                ? T('启动器仍在启动中，请稍后再试。', 'The launcher is still starting; try again shortly.')
                : j.error
            }
          } catch (_) { /* keep raw text */ }
          throw new Error(msg)
        })
      }
      return r.json()
    }).then(function (data) {
      if (okText) flash(okText)
      return data
    }).catch(function (e) { flash(T('失败: ', 'Failed: ') + (e.message || String(e)), 'err') })
  }

  function flash(text, kind) {
    var el = document.createElement('div')
    el.textContent = text
    el.style.cssText =
      'position:fixed;left:50%;top:16px;transform:translateX(-50%);z-index:2147483600;' +
      'padding:8px 14px;border-radius:8px;font-family:ui-sans-serif,system-ui,sans-serif;font-size:12px;' +
      'background:' + (kind === 'err' ? 'rgba(153,27,27,.95)' : 'rgba(20,83,45,.95)') +
      ';color:#fff;border:1px solid ' + (kind === 'err' ? 'rgba(248,113,113,.45)' : 'rgba(74,222,128,.45)') +
      ';box-shadow:0 6px 18px rgba(0,0,0,.35);'
    document.body.appendChild(el)
    setTimeout(function () { el.style.transition = 'opacity .3s'; el.style.opacity = '0'; setTimeout(function () { el.remove() }, 300) }, 2400)
  }

  // ---- Embedded terminal overlay ------------------------------------------
  var termOverlay = document.createElement('div')
  termOverlay.id = 'dshl-terminal-overlay'
  termOverlay.innerHTML =
    '<div id="dshl-terminal-header">' +
      '<div class="title">' +
        '<span class="dot" id="dshl-term-dot"></span>' +
        '<span id="dshl-term-title">' + T('内置终端', 'Embedded terminal') + '</span>' +
        '<span class="meta" id="dshl-term-meta">idle</span>' +
      '</div>' +
      '<div class="row">' +
        '<button class="btn" data-act="new-term">' + T('新建会话', 'New session') + '</button>' +
        '<button class="btn danger" data-act="kill-term">' + T('结束会话', 'Kill session') + '</button>' +
        '<button class="btn" data-act="close-term">' + T('关闭', 'Close') + '</button>' +
      '</div>' +
    '</div>' +
    '<div id="dshl-terminal-host"></div>' +
    '<div id="dshl-terminal-footer">Ready.</div>'
  termOverlay.addEventListener('click', function (e) {
    var t = e.target
    if (!(t instanceof HTMLElement)) return
    var act = t.getAttribute('data-act')
    if (act === 'close-term') closeTerminal()
    if (act === 'new-term') newTerminal(true)
    if (act === 'kill-term') killCurrentTerm()
  })
  document.body.appendChild(termOverlay)

  var currentTermState = null // {id, pid, ws, termObj (Xterm), fitAddon, observer, dot, meta, foot}
  function termEls() {
    return {
      dot: document.getElementById('dshl-term-dot'),
      title: document.getElementById('dshl-term-title'),
      meta: document.getElementById('dshl-term-meta'),
      host: document.getElementById('dshl-terminal-host'),
      foot: document.getElementById('dshl-terminal-footer'),
    }
  }
  function termFoot(msg) { var el = termEls().foot; if (el) el.textContent = msg }
  function setTermDot(alive) {
    var el = termEls().dot
    if (!el) return
    if (alive) el.classList.remove('off'); else el.classList.add('off')
  }

  function closeTerminal() { termOverlay.classList.remove('show'); tearDownCurrentTerm() }
  function openTerminal() {
    termOverlay.classList.add('show')
    if (!currentTermState) { newTerminal(false) } else { ensureLayout() }
  }

  function ensureLayout() {
    if (!currentTermState) return
    // Fit term into the current host size. Defer one frame so CSS layout has
    // settled (opening the overlay transitions display:none→flex).
    requestAnimationFrame(function () {
      try { currentTermState.fitAddon.fit() } catch (_) {}
    })
  }

  function tearDownCurrentTerm() {
    if (!currentTermState) return
    try { currentTermState.observer && currentTermState.observer.disconnect() } catch (_) {}
    try {
      if (currentTermState.ws && currentTermState.ws.readyState === WebSocket.OPEN) {
        currentTermState.ws.close(1000, 'tear down')
      }
    } catch (_) {}
    try { currentTermState.termObj.dispose() } catch (_) {}
    var host = termEls().host
    if (host) while (host.firstChild) host.removeChild(host.firstChild)
    setTermDot(false)
    currentTermState = null
  }

  function killCurrentTerm() {
    if (!currentTermState) return flash(T('无会话可结束', 'No session to kill'), 'err')
    var id = currentTermState.id
    fetch('/dshl-control/terminal/kill', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id: id }),
    }).then(function () {
      // The WS will get a close or error shortly; then retry shows it.
      tearDownCurrentTerm()
      termFoot(T('会话已结束', 'Session killed'))
    }).catch(function (e) { flash(T('kill 失败: ', 'kill failed: ') + (e.message || String(e)), 'err') })
  }

  function newTerminal(userTriggered) {
    tearDownCurrentTerm()
    var el = termEls()
    if (el.meta) el.meta.textContent = 'spawning…'
    termFoot('Creating PTY session via nativeCapabilities.terminal.spawn')

    // Create session with host dimensions. We open a minimal term first so
    // FitAddon can give us the real cols/rows we actually want.
    var cols0 = 100
    var rows0 = 24
    fetch('/dshl-control/terminal/spawn', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ cols: cols0, rows: rows0, cwd: locationCwdGuess() }),
    }).then(function (r) {
      if (!r.ok) return r.text().then(function (t) { throw new Error(t || ('HTTP ' + r.status)) })
      return r.json()
    }).then(function (info) {
      // info: { id, pid, wsUrl }  — from our index route wrapper
      loadTerminalSession(info.id, info.pid, info.wsUrl, cols0, rows0)
    }).catch(function (e) {
      if (el.meta) el.meta.textContent = 'spawn failed'
      termFoot(T('无法创建 PTY 会话：', 'Cannot create PTY session: ') + (e.message || String(e)))
      flash(T('spawn 失败: ', 'spawn failed: ') + (e.message || String(e)), 'err')
      setTermDot(false)
    })
    if (userTriggered) flash(T('正在创建新会话…', 'Spawning a new session…'))
  }

  function locationCwdGuess() {
    // If we knew the profile workspace we'd use it; fall back to an empty
    // string so the backend uses process.cwd(). Send explicit null so FFI
    // treat the field as None.
    return null
  }

  function loadTerminalSession(id, pid, wsUrl, colsInitial, rowsInitial) {
    // xterm.js dynamic import — runs after the import map is set.
    Promise.all([
      import('@xterm/xterm'),
      import('@xterm/addon-fit'),
    ]).then(function (mods) {
      var XtermModule = mods[0]
      var FitMod = mods[1]
      var Terminal = XtermModule.Terminal || XtermModule.default || XtermModule
      var FitAddon = FitMod.FitAddon || FitMod.default || FitMod

      var host = termEls().host
      if (!host) return
      var term = new Terminal({
        allowProposedApi: true,
        cursorBlink: true,
        fontSize: 13,
        lineHeight: 1.2,
        scrollback: 10000,
        convertEol: true,
        fontFamily: 'ui-monospace,SFMono-Regular,SF Mono,Menlo,Consolas,Liberation Mono,monospace',
        cols: colsInitial,
        rows: rowsInitial,
      })
      var fitAddon = new FitAddon()
      term.loadAddon(fitAddon)
      term.open(host)

      // WS connection & attach. Per experience 1514774 we do NOT touch
      // term.textarea, and resize is sent over WS as a BINARY JSON frame
      // (not terminal escape codes) so it can never collide with user
      // input on the text channel. xterm provides onData + fit() which
      // are the ONLY supported integration points.
      var ws = new WebSocket(wsUrl)
      ws.binaryType = 'arraybuffer'

      var state = {
        id: id,
        pid: pid,
        ws: ws,
        termObj: term,
        fitAddon: fitAddon,
        observer: null,
      }
      currentTermState = state
      setTermDot(true)
      var meta = termEls().meta
      if (meta) meta.textContent = 'pid ' + pid + ' · id ' + id.slice(0, 8)

      ws.addEventListener('message', function (ev) {
        if (typeof ev.data === 'string') {
          // Either a control init frame, or plain text data.
          if (ev.data.length && ev.data[0] === '{') {
            try {
              var parsed = JSON.parse(ev.data)
              if (parsed && parsed.t === 'init') {
                termFoot('Connected to session ' + parsed.id + ' pid ' + parsed.pid)
                return
              }
            } catch (_) { /* not JSON → fallthrough to write */ }
          }
          term.write(ev.data)
          return
        }
        // Binary / arraybuffer → Uint8Array.
        try {
          var bytes = new Uint8Array(ev.data)
          // writeUtf8 via TextDecoder → string; xterm.write accepts string
          var s = new TextDecoder().decode(bytes)
          term.write(s)
        } catch (_) {}
      })
      ws.addEventListener('open', function () {
        termFoot('WebSocket connected (dshl-core PTY)')
        // Now that host is visible, fit to true css size; notify PTY.
        ensureLayout()
      })
      ws.addEventListener('close', function () {
        setTermDot(false)
        termFoot('Session closed')
      })
      ws.addEventListener('error', function () {
        setTermDot(false)
        termFoot('WebSocket connection error')
        flash(T('终端连接错误', 'Terminal connection error'), 'err')
      })

      // stdin → ws. Text frames are unconditionally shell input on the
      // server (pasted JSON is never intercepted); control ops ride
      // binary frames only.
      term.onData(function (s) {
        if (ws.readyState !== WebSocket.OPEN) return
        ws.send(s)
      })

      // Resize: drive by ResizeObserver on the host (better than window
      // resize events, since the overlay itself can be drag/maximized by
      // user in future versions).
      try {
        state.observer = new ResizeObserver(function () {
          try {
            var dims = fitAddon.proposeDimensions()
            if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows)) return
            fitAddon.fit()
            if (ws.readyState === WebSocket.OPEN) {
              ws.send(new TextEncoder().encode(
                JSON.stringify({ op: 'resize', cols: term.cols, rows: term.rows })))
            }
          } catch (_) {}
        })
        state.observer.observe(host)
      } catch (_) { /* ResizeObserver optional */ }

      // Initial alignment: if computed CSS gives us a size, use it now.
      setTimeout(function () { ensureLayout() }, 0)
    }).catch(function (e) {
      termFoot('xterm.js import failed: ' + (e.message || String(e)))
      flash(T('xterm.js 加载失败: ', 'xterm.js load failed: ') + (e.message || String(e)), 'err')
    })
  }

  // ---- Guard overlay --------------------------------------------------------
  var overlay = document.createElement('div')
  overlay.id = 'dshl-control-overlay'
  overlay.innerHTML =
    '<div id="dshl-control-panel">' +
      '<header>' +
        '<h3>' + T('DSHL Guard 插件守护', 'DSHL Guard plugin guard') + '</h3>' +
        '<div class="row"><button class="btn alt" data-act="mark-healthy">Mark healthy</button>' +
        '<button class="btn danger" data-act="mark-failed">Mark failed</button>' +
        '<button class="close">' + T('关闭', 'Close') + '</button></div>' +
      '</header>' +
      '<div class="row" id="dshl-guard-stats"></div>' +
      '<section id="dshl-guard-rollback"></section>' +
      '<section>' +
        '<h4>' + T('插件列表 / 手动启用禁用', 'Plugins / manual enable-disable') + '</h4>' +
        '<ul class="plugins" id="dshl-guard-list"></ul>' +
        '<p class="muted">' + T(
          '保护插件（@dshl/control 自身）不允许禁用。禁用状态会被持久化记录；注意 dsh 加载器目前尚不读取该列表——实际跳过加载需要上游支持。',
          'The guard plugin itself (@dshl/control) cannot be disabled. Disable state is persisted; note the dsh plugin loader does not consume this list yet — actually skipping loads needs upstream support.') + '</p>' +
      '</section>' +
    '</div>'
  overlay.addEventListener('click', function (e) {
    if (e.target === overlay) closeOverlay()
    var t = e.target
    if (!(t instanceof HTMLElement)) return
    if (t.classList.contains('close')) closeOverlay()
    var act = t.getAttribute('data-act')
    if (act === 'mark-healthy') post(GUARD_MARK_HEALTHY, { bundles: guardBundleSnapshot }, T('已标记健康', 'Marked healthy')).then(refreshAll)
    if (act === 'mark-failed') post(GUARD_MARK_FAILED, { report: 'user-manual-failed' }, T('已标记失败', 'Marked failed')).then(refreshAll)
    var disableBtn = t.getAttribute('data-disable')
    if (disableBtn) {
      var reason = prompt(T('禁用原因（留空=manual）：', 'Disable reason (empty=manual):'))
      if (reason === null) return
      post(GUARD_ACTION(disableBtn, 'disable'), { reason: reason || 'manual' }, T('已禁用 ', 'Disabled ') + disableBtn).then(refreshAll)
    }
    var enableBtn = t.getAttribute('data-enable')
    if (enableBtn) post(GUARD_ACTION(enableBtn, 'enable'), null, T('已启用 ', 'Enabled ') + enableBtn).then(refreshAll)
  })
  document.body.appendChild(overlay)

  function openOverlay() { overlay.classList.add('show'); refreshAll() }
  function closeOverlay() { overlay.classList.remove('show') }

  var guardStateSnapshot = null
  var guardBundleSnapshot = []
  function refreshAll() {
    Promise.all([
      fetch(GUARD_LIST).then(function (r) { return r.json() }),
      fetch(GUARD_ROLLBACK).then(function (r) { return r.json() }),
    ]).then(function (vs) {
      var list = vs[0]
      var rb = vs[1]
      var stats = document.getElementById('dshl-guard-stats')
      var rbBox = document.getElementById('dshl-guard-rollback')
      var lsBox = document.getElementById('dshl-guard-list')
      if (stats) {
        var crashes = rb && Number.isFinite(rb.consecutiveCrashes) ? rb.consecutiveCrashes : 0
        var threshold = rb && rb.autoDisableThreshold ? rb.autoDisableThreshold : 3
        var disabled = (list && Array.isArray(list.bundles) ? list.bundles.filter(function (b) { return b.status === 'disabled' }) : [])
        stats.innerHTML =
          statBox(T('连续崩溃次数', 'Consecutive crashes'), String(crashes) + ' / ' + String(threshold) + T(' 阈值', ' threshold'),
            crashes >= threshold ? 'bad' : crashes > 0 ? 'warn' : 'ok') +
          statBox(T('禁用插件', 'Disabled plugins'), String(disabled.length), disabled.length ? 'bad' : 'ok') +
          statBox(T('当前状态', 'Status'), (rb && rb.healthy) ? 'healthy' : 'pending', rb && rb.healthy ? 'ok' : 'warn') +
          statBox(T('上次健康', 'Last healthy'), (rb && rb.lastHealthyAt) ? shortAgo(rb.lastHealthyAt) : 'never',
            (rb && rb.lastHealthyAt) ? 'ok' : 'warn')
      }
      if (rbBox) {
        var html = '<h4>' + T('崩溃回滚状态', 'Crash rollback state') + '</h4>'
        if (rb && rb.rollback && rb.rollback.enabled) {
          html += '<div class="pill disabled">' + T('已执行自动回滚', 'Auto rollback executed') + '</div>'
          html += '<p class="muted">' + T(
            '检测到上次启动未在健康窗口期内上报 healthy，且连续崩溃次数达阈值，已把新增/可疑插件写入禁用记录：',
            'The previous start never reported healthy within the window and the crash counter hit the threshold; suspicious/new plugins were written to the disable record: ')
          if (rb.rollback.suspicious && rb.rollback.suspicious.length) {
            html += ' <span class="suspicious">' + escapeHtml(rb.rollback.suspicious.join(', ')) + '</span>'
          } else {
            html += ' ' + T('无可疑包（可能是新老 bundles 集合完全一致）。', 'No suspicious packages (the bundle sets may be identical).')
          }
          html += ' ' + T(
            '该记录仅为状态跟踪——dsh 加载器目前不读取它，实际跳过加载需上游支持。',
            'This record is tracking only — the dsh plugin loader does not consume it yet; actually skipping loads needs upstream support.') + '</p>'
        } else if (rb && rb.lastHealthyBundles && Array.isArray(rb.lastHealthyBundles)) {
          html += '<div class="pill active">' + T('健康快照存在', 'Healthy snapshot exists') + '</div>'
          html += '<p class="muted">' + T(
            '健康 snapshot 含 ' + rb.lastHealthyBundles.length + ' 个 bundle。下次若启动失败会对比差异记录回滚。',
            'Healthy snapshot covers ' + rb.lastHealthyBundles.length + ' bundles. A failed start will diff against it for the rollback record.') + '</p>'
        } else {
          html += '<div class="pill protected">' + T('尚无健康快照', 'No healthy snapshot yet') + '</div>'
          html += '<p class="muted">' + T(
            '首次启动或上次未调用 mark-healthy。点击 Mark healthy 保存 snapshot。',
            'First start, or the last run never called mark-healthy. Click Mark healthy to save a snapshot.') + '</p>'
        }
        if (rb && rb.startedAt) html += '<p class="muted">' + T('本次启动时间：', 'Started at: ') + new Date(rb.startedAt).toLocaleString() + '</p>'
        rbBox.innerHTML = html
      }
      if (lsBox) {
        var bundles = list && Array.isArray(list.bundles) ? list.bundles : []
        guardBundleSnapshot = bundles.map(function (b) { return b.packageName })
        var rows = ''
        if (!bundles.length) rows = '<li><div class="muted">' + T('暂无可用 bundles。尝试在 profile 中加载至少一个插件。', 'No bundles available. Load at least one plugin in the profile.') + '</div><div></div></li>'
        bundles.forEach(function (b) {
          var pillClass = b.status === 'protected' ? 'protected' : (b.status === 'disabled' ? 'disabled' : 'active')
          // meta is RAW text here; escaped exactly once at insertion below
          // (double-escaping used to render literal &amp; entities).
          var meta = ''
          if (b.disabledReason) meta += b.disabledReason
          if (b.disabledAt) meta += (meta ? ' · ' : '') + new Date(b.disabledAt).toLocaleString()
          rows += '<li>' +
            '<div>' +
              '<span class="pill ' + pillClass + '">' + escapeHtml(b.status) + '</span>' +
              '<strong>' + escapeHtml(b.packageName) + '</strong>' +
              (meta ? '<div class="meta">' + escapeHtml(meta) + '</div>' : '') +
            '</div>' +
            '<div class="actions">' +
              (b.mutable && b.status !== 'disabled'
                ? '<button class="secondary" data-disable="' + escapeAttr(b.packageName) + '">' + T('禁用', 'Disable') + '</button>'
                : '') +
              (b.mutable && b.status === 'disabled'
                ? '<button data-enable="' + escapeAttr(b.packageName) + '">' + T('启用', 'Enable') + '</button>'
                : '') +
            '</div>' +
          '</li>'
        })
        lsBox.innerHTML = rows
      }
    }).catch(function (e) { flash(T('Guard 状态读取失败: ', 'Guard state read failed: ') + (e.message || String(e)), 'err') })
  }

  function statBox(k, v, kind) {
    var color = '#f8fafc'
    if (kind === 'bad') color = '#fecaca'
    else if (kind === 'warn') color = '#fde68a'
    else if (kind === 'ok') color = '#a7f3d0'
    return '<div class="stat"><div class="k">' + escapeHtml(k) + '</div><div class="v" style="color:' + color + '">' + escapeHtml(v) + '</div></div>'
  }

  function shortAgo(iso) {
    var t = Date.parse(iso); if (!Number.isFinite(t)) return String(iso)
    var diff = Math.max(0, Math.floor((Date.now() - t) / 1000))
    if (diff < 60) return diff + 's ago'
    if (diff < 3600) return Math.floor(diff / 60) + 'm ago'
    if (diff < 86400) return Math.floor(diff / 3600) + 'h ago'
    return Math.floor(diff / 86400) + 'd ago'
  }

  function escapeHtml(s) { return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c] }) }
  function escapeAttr(s) { return escapeHtml(s) }

  // ---- Attach buttons to bar ------------------------------------------------
  bar.appendChild(button(T('终端', 'Terminal'), openTerminal))
  bar.appendChild(button(T('系统终端', 'System terminal'), function () { post('/dshl-control/open-terminal', {}, T('已请求打开系统终端', 'System terminal requested')) }))
  bar.appendChild(button(T('重启', 'Restart'), function () { post('/dshl-control/restart', {}, T('重启请求已发送', 'Restart requested')) }))
  bar.appendChild(button(T('关机', 'Shutdown'), function () { post('/dshl-control/shutdown', {}, T('关机请求已发送', 'Shutdown requested')) }))

  var guardBtn = button('Guard', openOverlay)
  badgeEl = document.createElement('span')
  badgeEl.className = 'badge'
  badgeEl.textContent = '0'
  badgeEl.style.display = 'none'
  guardBtn.appendChild(badgeEl)
  bar.appendChild(guardBtn)

  document.body.appendChild(bar)

  // ---- Periodic state refresh (updates the corner badge with rollback state)
  function refreshBadge() {
    fetch(STATE_URL).then(function (r) { return r.json() }).then(function (s) {
      if (!guardOverlayShowing()) {
        // Skip full fetch unless badge needs updating? Always allow.
      }
      var g = s && s.guard ? s.guard : null
      if (!g || !badgeEl) return
      var rb = g.rollback || {}
      var crashes = Number.isFinite(rb.consecutiveCrashes) ? rb.consecutiveCrashes : 0
      var disabled = typeof g.disabledCount === 'number' ? g.disabledCount : 0
      var show = crashes > 0 || disabled > 0
      badgeEl.style.display = show ? 'inline-flex' : 'none'
      if (show) {
        badgeEl.textContent = String(disabled + crashes)
        badgeEl.classList.remove('ok', 'warn')
        if (rb.rollback && rb.rollback.enabled) badgeEl.className = 'badge' // red default
        else if (crashes > 0) badgeEl.classList.add('warn')
        else badgeEl.classList.add('ok')
      }
      guardStateSnapshot = g
    }).catch(function () { /* offline */ })
  }
  function guardOverlayShowing() { return overlay.classList.contains('show') }

  refreshBadge()
  setInterval(refreshBadge, 15000)

  // Auto mark-healthy, ONCE, after the widget has been stably running for a
  // minute: the renderer being alive IS the health signal the guard wants.
  // Without this nothing ever calls mark-healthy (the button is manual), so
  // lastHealthyBundles stays null forever and crash rollback can never fire.
  // The 60s stability bar (was 3s) matters: a dsh that dies at t=10s must
  // still count as a failed start for the rollback state — marking healthy
  // after 3s would wipe the crash signal for anything that survives those
  // first seconds. Fetching the live bundle list first keeps the snapshot
  // accurate; failure here is non-fatal (guard stays "pending", manual
  // button still works).
  setTimeout(function () {
    fetch(GUARD_LIST).then(function (r) { return r.json() }).then(function (list) {
      var bundles = list && Array.isArray(list.bundles)
        ? list.bundles.map(function (b) { return b.packageName })
        : []
      return post(GUARD_MARK_HEALTHY, { bundles: bundles }, '')
    }).catch(function () { /* renderer not authorized or guard absent */ })
  }, 60000)
})()
