// @dshl/control web widget: a small floating launcher action bar injected
// into the dsh web UI by the bundle's index tap. Each button calls the
// plugin's own local HTTP route, which forwards to dshl over the control pipe.
(function () {
  'use strict'
  if (window.__dshlControlUiLoaded) return
  window.__dshlControlUiLoaded = true

  var style = document.createElement('style')
  style.textContent =
    '#dshl-control-bar{position:fixed;right:16px;bottom:16px;z-index:2147483000;display:flex;gap:8px;font-family:ui-sans-serif,system-ui,sans-serif;}' +
    '#dshl-control-bar button{appearance:none;border:1px solid rgba(127,127,127,.35);background:rgba(24,24,27,.92);color:#e4e4e7;border-radius:9999px;padding:6px 14px;font-size:13px;line-height:1;cursor:pointer;box-shadow:0 2px 8px rgba(0,0,0,.25);transition:background .15s ease;}' +
    '#dshl-control-bar button:hover{background:#27272a;}' +
    '#dshl-control-bar button:active{transform:translateY(1px);}'
  document.head.appendChild(style)

  var bar = document.createElement('div')
  bar.id = 'dshl-control-bar'

  function button(label, path) {
    var el = document.createElement('button')
    el.type = 'button'
    el.textContent = label
    el.addEventListener('click', function () {
      fetch(path, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: '{}',
      }).catch(function () {})
    })
    return el
  }

  bar.appendChild(button('开终端', '/dshl-control/open-terminal'))
  bar.appendChild(button('重启', '/dshl-control/restart'))
  bar.appendChild(button('关机', '/dshl-control/shutdown'))
  document.body.appendChild(bar)
})()