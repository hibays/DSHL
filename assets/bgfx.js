// DSHL startup page — the background art layer (see DESIGN.md "Background
// Art Layer"): a GPU particle fluid cloud behind the Swiss console.
//
// Architecture (raw WebGL2, zero dependencies — WebView2/Chromium class):
//   * ~124k particles live as positions in an RGBA16F "position texture"
//     (xyz = world pos in a 400×400×200 box, w = density).
//   * A SIM pass (fullscreen quad → ping-pong FBO) integrates positions
//     every frame with Euler steps along a 3-scale simplex flow field —
//     particles genuinely advect: clouds gather, tumble and dissolve.
//     The mouse is a repulsion force in world space (the air-poke).
//   * A DRAW pass renders gl.POINTS straight from the texture in ONE draw
//     call: hard SQUARE dots (1px feather), matte normal blending, constant
//     point size, static per-particle brightness dither — the dithered
//     dot-matrix halftone of the TRAE reference: gray-white smoke, mint
//     cores, true voids. Density is SLOW noise only (no speed term) and
//     size never attenuates — the two anti-flicker invariants.
//   * The camera parallaxes with the pointer (≤1.5% of viewport).
// Tiers: small viewports / weak GPUs drop to ~31k particles and 2 octaves;
// DPR is capped at 2; no WebGL2 → the layer quietly leaves a static CSS
// fog and the wordmark; hidden tab pauses the loop; prefers-reduced-motion
// warms the simulation once and shows a single still frame.

"use strict";

(function () {
  const layer = document.getElementById("bgfx");
  const canvas = document.getElementById("bgfx-canvas");
  if (!layer || !canvas) return;

  // ── Tuning (density ramp stops live in the shaders' uniforms) ─────────
  const BOX = { x: 200, y: 200, z: 100 }; // world half-extents (400×400×200)
  const CAM_Z = 330; // camera distance to z=0 plane
  const FOV = 58;
  const PARALLAX = { x: 7, y: 5 }; // world units at ±1 pointer offset
  const REPEL_RADIUS = 85; // mouse repulsion, world units
  const REPEL_STRENGTH = 2.6;
  const STEP = 11; // Euler step scale (world units per second-ish)
  const WARMUP = 70; // sim frames run once at boot so clouds arrive evolved

  const THEMES = {
    // The TRAE reference is a dithered dot-matrix: matte gray-white
    // squares, the densest cores tinted mint, whole regions left void.
    // Normal (non-additive) blending — glow would kill the halftone read.
    dark: {
      stops: [
        [0.0, [0.10, 0.11, 0.12]], // near-black floor (culled anyway)
        [0.35, [0.58, 0.61, 0.63]], // smoke gray
        [0.65, [0.84, 0.86, 0.87]], // bright gray-white
        [1.0, [0.36, 0.80, 0.63]], // mint core (TRAE green family)
      ],
      alpha: 1.0,
      size: 2.4,
    },
    light: {
      stops: [
        [0.0, [0.92, 0.93, 0.94]],
        [0.35, [0.55, 0.57, 0.58]], // mid gray on white
        [0.65, [0.24, 0.26, 0.27]], // near-black dots
        [1.0, [0.00, 0.62, 0.44]], // deep green core
      ],
      alpha: 0.85,
      size: 2.6,
    },
  };

  const pointer = { x: 0, y: 0, on: false, nx: 0, ny: 0, wx: 0, wy: 0, wz: 0 };
  const darkMq = window.matchMedia
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : null;
  const reduceMq = window.matchMedia
    ? window.matchMedia("(prefers-reduced-motion: reduce)")
    : null;

  const SIM_VS = `#version 300 es
  in vec2 aPos;
  out vec2 vUv;
  void main() { vUv = aPos * 0.5 + 0.5; gl_Position = vec4(aPos, 0.0, 1.0); }`;

  // Euler advection through a 3-scale simplex flow field (Ashima snoise).
  const SIM_FS = `#version 300 es
  precision highp float;
  in vec2 vUv;
  out vec4 outPos;
  uniform sampler2D uPos;
  uniform float uDt;
  uniform float uTime;
  uniform vec3 uMouse;
  uniform float uMouseOn;
  uniform float uOctaves;
  uniform vec3 uBox;

  vec3 mod289(vec3 x){ return x - floor(x * (1.0/289.0)) * 289.0; }
  vec4 mod289(vec4 x){ return x - floor(x * (1.0/289.0)) * 289.0; }
  vec4 permute(vec4 x){ return mod289(((x*34.0)+1.0)*x); }
  vec4 taylorInvSqrt(vec4 r){ return 1.79284291400159 - 0.85373472095314 * r; }
  float snoise(vec3 v){
    const vec2 C = vec2(1.0/6.0, 1.0/3.0);
    const vec4 D = vec4(0.0, 0.5, 1.0, 2.0);
    vec3 i  = floor(v + dot(v, C.yyy));
    vec3 x0 = v - i + dot(i, C.xxx);
    vec3 g = step(x0.yzx, x0.xyz);
    vec3 l = 1.0 - g;
    vec3 i1 = min(g.xyz, l.zxy);
    vec3 i2 = max(g.xyz, l.zxy);
    vec3 x1 = x0 - i1 + C.xxx;
    vec3 x2 = x0 - i2 + C.yyy;
    vec3 x3 = x0 - D.yyy;
    i = mod289(i);
    vec4 p = permute(permute(permute(
              i.z + vec4(0.0, i1.z, i2.z, 1.0))
            + i.y + vec4(0.0, i1.y, i2.y, 1.0))
            + i.x + vec4(0.0, i1.x, i2.x, 1.0));
    float n_ = 0.142857142857;
    vec3 ns = n_ * D.wyz - D.xzx;
    vec4 j = p - 49.0 * floor(p * ns.z * ns.z);
    vec4 x_ = floor(j * ns.z);
    vec4 y_ = floor(j - 7.0 * x_);
    vec4 x = x_ * ns.x + ns.yyyy;
    vec4 y = y_ * ns.x + ns.yyyy;
    vec4 h = 1.0 - abs(x) - abs(y);
    vec4 b0 = vec4(x.xy, y.xy);
    vec4 b1 = vec4(x.zw, y.zw);
    vec4 s0 = floor(b0) * 2.0 + 1.0;
    vec4 s1 = floor(b1) * 2.0 + 1.0;
    vec4 sh = -step(h, vec4(0.0));
    vec4 a0 = b0.xzyw + s0.xzyw * sh.xxyy;
    vec4 a1 = b1.xzyw + s1.xzyw * sh.zzww;
    vec3 p0 = vec3(a0.xy, h.x);
    vec3 p1 = vec3(a0.zw, h.y);
    vec3 p2 = vec3(a1.xy, h.z);
    vec3 p3 = vec3(a1.zw, h.w);
    vec4 norm = taylorInvSqrt(vec4(dot(p0,p0), dot(p1,p1), dot(p2,p2), dot(p3,p3)));
    p0 *= norm.x; p1 *= norm.y; p2 *= norm.z; p3 *= norm.w;
    vec4 m = max(0.6 - vec4(dot(x0,x0), dot(x1,x1), dot(x2,x2), dot(x3,x3)), 0.0);
    m = m * m;
    return 42.0 * dot(m*m, vec4(dot(p0,x0), dot(p1,x1), dot(p2,x2), dot(p3,x3)));
  }

  void main() {
    vec4 p = texture(uPos, vUv);
    vec3 pos = p.xyz;
    vec3 q = pos * 0.008;
    // Large scale: the global weather. Medium: the clumps. Small: fringe
    // turbulence. Each component of the flow comes from its own noise
    // lookup so the field is divergent-free-ish and clouds really travel.
    vec3 v;
    v.x = snoise(vec3(q.x + uTime * 0.045, q.y, q.z));
    v.y = snoise(vec3(q.x, q.y + 13.7, q.z + uTime * 0.055));
    v.z = snoise(vec3(q.x - 7.3 + uTime * 0.035, q.y, q.z)) * 0.55;
    if (uOctaves > 1.5) {
      vec3 q2 = q * 3.1 + 19.0;
      v += 0.42 * vec3(
        snoise(vec3(q2.x, q2.y + uTime * 0.09, q2.z)),
        snoise(vec3(q2.x + uTime * 0.08, q2.y, q2.z + 5.0)),
        snoise(vec3(q2.x, q2.y + 5.0, q2.z + uTime * 0.07)));
    }
    if (uOctaves > 2.5) {
      vec3 q3 = q * 8.7 + 43.0;
      v += 0.18 * vec3(
        snoise(vec3(q3.xy + uTime * 0.16, q3.z)),
        snoise(vec3(q3.yz + uTime * 0.14, q3.x)),
        snoise(vec3(q3.zx + uTime * 0.12, q3.y)));
    }
    // Mouse repulsion: a local inverse field — the pointer pokes the air.
    if (uMouseOn > 0.5) {
      vec3 d = pos - uMouse;
      float r2 = dot(d, d);
      float f = exp(-r2 / (2.0 * ${REPEL_RADIUS}.0 * ${REPEL_RADIUS}.0));
      v += (d / max(sqrt(r2), 0.001)) * f * ${REPEL_STRENGTH};
    }
    pos += v * uDt * ${STEP}.0;
    // Micro-diffusion: a sub-pixel jitter each step keeps the flow from
    // shearing the field into ever-thinner filaments (advection alone
    // collapses clouds into strings within a minute).
    vec3 jit = vec3(
      snoise(pos * 0.11 + uTime * 0.7),
      snoise(pos * 0.11 + 17.0 + uTime * 0.6),
      snoise(pos * 0.11 + 41.0 + uTime * 0.5));
    pos += jit * 1.35;
    // Wrap inside the box (clouds leaving one wall re-enter the other).
    pos = mod(pos + uBox, 2.0 * uBox) - uBox;
    // Density for the colour ramp — SLOW noise ONLY. No flow-speed term:
    // a per-frame brightness component is exactly what made the field
    // flicker. The large-scale mask carves true voids (the reference is
    // smoke bands over black, not uniform haze).
    float n = snoise(vec3(q * 1.4 + uTime * 0.05)) * 0.5 + 0.5;
    float m = snoise(vec3(q * 0.85 + uTime * 0.028 + 31.0)) * 0.5 + 0.5;
    m = smoothstep(0.22, 0.52, m);
    // Recycle: particles stranded in a void respawn at a fresh scatter
    // position, so particle density continuously tracks the weather mask
    // and clouds stay BROAD (the reference is cloud masses, not strings).
    float rnd = fract(sin(dot(vUv, vec2(12.9898, 78.233)) + uTime * 0.61) * 43758.5453);
    if (rnd < (1.0 - m) * 0.02) {
      vec3 r = vec3(
        fract(sin(dot(vUv, vec2(39.34, 11.13)) + uTime * 1.7)),
        fract(sin(dot(vUv, vec2(71.13, 27.79)) + uTime * 1.3)),
        fract(sin(dot(vUv, vec2(93.89, 47.61)) + uTime * 2.1)));
      pos = (r * 2.0 - 1.0) * uBox;
    }
    outPos = vec4(pos, clamp(m * (0.55 + 0.6 * n), 0.0, 1.0));
  }`;

  const DRAW_VS = `#version 300 es
  precision highp float;
  uniform sampler2D uPos;
  uniform int uTexW;
  uniform mat4 uProj;
  uniform mat4 uView;
  uniform vec2 uPar;
  uniform float uSize;
  uniform float uFragH;
  out float vDensity;
  out float vHash;
  void main() {
    int id = gl_VertexID;
    ivec2 tc = ivec2(id % uTexW, id / uTexW);
    vec4 p = texelFetch(uPos, tc, 0);
    vec4 clip = uProj * uView * vec4(p.xyz + vec3(uPar, 0.0), 1.0);
    gl_Position = clip;
    // CONSTANT point size — perspective attenuation made far particles
    // pop across the 1px clamp boundary (flicker source #2).
    gl_PointSize = uSize;
    vDensity = p.w;
    // Static per-particle hash: a FIXED brightness jitter per dot. The
    // dither that turns a smooth field into granular smoke — static,
    // therefore flicker-free.
    vHash = fract(sin(float(id) * 127.1) * 43758.5453);
  }`;

  const DRAW_FS = `#version 300 es
  precision highp float;
  in float vDensity;
  in float vHash;
  out vec4 outColor;
  uniform float uAlpha;
  uniform vec3 uC0; uniform vec3 uC1; uniform vec3 uC2; uniform vec3 uC3;
  vec3 ramp(float t) {
    t = clamp(t, 0.0, 1.0);
    vec3 c = mix(uC0, uC1, smoothstep(0.0, 0.3, t));
    c = mix(c, uC2, smoothstep(0.3, 0.55, t));
    c = mix(c, uC3, smoothstep(0.85, 0.98, t));
    return c;
  }
  void main() {
    // Hard SQUARE dot with a 1px feather — the halftone read. No radial
    // falloff: soft sprites at 1-2px blink whole-dot (flicker source #3).
    vec2 pc = abs(gl_PointCoord - 0.5);
    float a = 1.0 - smoothstep(0.35, 0.5, max(pc.x, pc.y));
    float density = clamp(vDensity * 1.25, 0.0, 1.0);
    // Fixed dither + a cull floor: the faintest dots vanish instead of
    // shimmering at the alpha noise floor.
    float bright = density * mix(0.6, 1.0, vHash);
    float vis = step(0.07, bright);
    outColor = vec4(ramp(density), a * uAlpha * bright * vis);
  }`;

  function compile(gl, type, src) {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      throw new Error("shader: " + gl.getShaderInfoLog(s));
    }
    return s;
  }
  function program(gl, vs, fs) {
    const p = gl.createProgram();
    gl.attachShader(p, compile(gl, gl.VERTEX_SHADER, vs));
    gl.attachShader(p, compile(gl, gl.FRAGMENT_SHADER, fs));
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
      throw new Error("link: " + gl.getProgramInfoLog(p));
    }
    return p;
  }

  let gl = null;
  let simProg, drawProg, simVao, drawVao, quadBuf;
  let texA, texB, fboA, fboB, texW;
  let octaves = 3;
  let running = false;
  let raf = 0;
  let last = 0;
  let theme = THEMES.dark;

  function initGL() {
    gl = canvas.getContext("webgl2", { alpha: true, antialias: false, depth: false });
    if (!gl) throw new Error("no webgl2");
    if (!gl.getExtension("EXT_color_buffer_float")) {
      // Half-float render targets keep the ping-pong possible on GPUs
      // without full float support.
      if (!gl.getExtension("EXT_color_buffer_half_float")) throw new Error("no float rt");
    }
    // Particle tier: desktop 352²≈124k, laptop/small 256²≈65k, weak/mobile
    // 176²≈31k (and the smallest tier also drops the third noise octave).
    const area = window.innerWidth * window.innerHeight;
    if (area >= 1100000) texW = 352;
    else if (area >= 500000) texW = 256;
    else { texW = 176; octaves = 2; }

    // Fullscreen quad for the sim pass.
    quadBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, quadBuf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    simVao = gl.createVertexArray();
    gl.bindVertexArray(simVao);
    simProg = program(gl, SIM_VS, SIM_FS);
    const aPos = gl.getAttribLocation(simProg, "aPos");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);

    // Draw pass needs no attributes (gl_VertexID addressing) — empty VAO.
    drawVao = gl.createVertexArray();
    drawProg = program(gl, DRAW_VS, DRAW_FS);

    // Position textures, seeded with a uniform scatter in the box.
    const n = texW * texW;
    const seeds = new Float32Array(n * 4);
    for (let i = 0; i < n; i++) {
      seeds[i * 4 + 0] = (Math.random() * 2 - 1) * BOX.x;
      seeds[i * 4 + 1] = (Math.random() * 2 - 1) * BOX.y;
      seeds[i * 4 + 2] = (Math.random() * 2 - 1) * BOX.z;
      seeds[i * 4 + 3] = Math.random();
    }
    const mkTex = () => {
      const t = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, t);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA16F, texW, texW, 0, gl.RGBA, gl.FLOAT, seeds);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      return t;
    };
    texA = mkTex();
    texB = mkTex();
    const mkFbo = (t) => {
      const f = gl.createFramebuffer();
      gl.bindFramebuffer(gl.FRAMEBUFFER, f);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, t, 0);
      return f;
    };
    fboA = mkFbo(texA);
    fboB = mkFbo(texB);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    if (gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) {
      // Re-check with a bound fbo (the null rebind above reset it).
      gl.bindFramebuffer(gl.FRAMEBUFFER, fboA);
      const ok = gl.checkFramebufferStatus(gl.FRAMEBUFFER) === gl.FRAMEBUFFER_COMPLETE;
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      if (!ok) throw new Error("fbo incomplete");
    }
  }

  function resize() {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.round(window.innerWidth * dpr);
    canvas.height = Math.round(window.innerHeight * dpr);
  }

  // Minimal mat4 helpers: perspective and a translating view.
  function perspective(fovy, aspect, near, far) {
    const f = 1 / Math.tan(fovy / 2);
    const nf = 1 / (near - far);
    return new Float32Array([
      f / aspect, 0, 0, 0,
      0, f, 0, 0,
      0, 0, (far + near) * nf, -1,
      0, 0, 2 * far * near * nf, 0,
    ]);
  }

  function simStep(dt, time) {
    gl.bindFramebuffer(gl.FRAMEBUFFER, fboB);
    gl.viewport(0, 0, texW, texW);
    gl.useProgram(simProg);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, texA);
    gl.uniform1i(gl.getUniformLocation(simProg, "uPos"), 0);
    gl.uniform1f(gl.getUniformLocation(simProg, "uDt"), dt);
    gl.uniform1f(gl.getUniformLocation(simProg, "uTime"), time);
    gl.uniform3f(gl.getUniformLocation(simProg, "uMouse"), pointer.wx, pointer.wy, pointer.wz);
    gl.uniform1f(gl.getUniformLocation(simProg, "uMouseOn"), pointer.on ? 1 : 0);
    gl.uniform1f(gl.getUniformLocation(simProg, "uOctaves"), octaves);
    gl.uniform3f(gl.getUniformLocation(simProg, "uBox"), BOX.x, BOX.y, BOX.z);
    gl.bindVertexArray(simVao);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    const t = texA; texA = texB; texB = t;
    const f = fboA; fboA = fboB; fboB = f;
  }

  function draw(time) {
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    // Matte, premultiplied-free normal blending — additive glow would
    // destroy the halftone dot-matrix look.
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.enable(gl.BLEND);
    gl.useProgram(drawProg);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, texA);
    gl.uniform1i(gl.getUniformLocation(drawProg, "uPos"), 0);
    gl.uniform1i(gl.getUniformLocation(drawProg, "uTexW"), texW);
    const aspect = canvas.width / canvas.height;
    const proj = perspective((FOV * Math.PI) / 180, aspect, 60, 1600);
    gl.uniformMatrix4fv(gl.getUniformLocation(drawProg, "uProj"), false, proj);
    // Camera parallaxes with the pointer — ≤1.5% of the viewport.
    const px = pointer.nx * PARALLAX.x;
    const py = pointer.ny * PARALLAX.y;
    const view = new Float32Array([
      1, 0, 0, 0,
      0, 1, 0, 0,
      0, 0, 1, 0,
      px, py, -CAM_Z, 1,
    ]);
    gl.uniformMatrix4fv(gl.getUniformLocation(drawProg, "uView"), false, view);
    gl.uniform2f(gl.getUniformLocation(drawProg, "uPar"), 0, 0);
    gl.uniform1f(gl.getUniformLocation(drawProg, "uSize"), theme.size);
    gl.uniform1f(gl.getUniformLocation(drawProg, "uFragH"), canvas.height);
    gl.uniform1f(gl.getUniformLocation(drawProg, "uAlpha"), theme.alpha);
    const s = theme.stops;
    gl.uniform3f(gl.getUniformLocation(drawProg, "uC0"), s[0][1][0], s[0][1][1], s[0][1][2]);
    gl.uniform3f(gl.getUniformLocation(drawProg, "uC1"), s[1][1][0], s[1][1][1], s[1][1][2]);
    gl.uniform3f(gl.getUniformLocation(drawProg, "uC2"), s[2][1][0], s[2][1][1], s[2][1][2]);
    gl.uniform3f(gl.getUniformLocation(drawProg, "uC3"), s[3][1][0], s[3][1][1], s[3][1][2]);
    gl.bindVertexArray(drawVao);
    gl.drawArrays(gl.POINTS, 0, texW * texW);
    gl.bindVertexArray(null);
  }

  function frame(now) {
    const dt = Math.min(3, Math.max(0.1, (now - last) / 16.7));
    last = now;
    simStep(dt, now / 1000);
    draw(now);
    raf = requestAnimationFrame(frame);
  }

  function start() {
    last = performance.now();
    raf = requestAnimationFrame(frame);
  }
  function stop() {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
  }

  // Unproject the pointer onto the z=0 plane (the mouse's world position).
  function updateMouseWorld() {
    const halfH = Math.tan((FOV * Math.PI) / 360) * CAM_Z;
    const halfW = halfH * (window.innerWidth / window.innerHeight);
    pointer.wx = pointer.nx * halfW;
    pointer.wy = pointer.ny * halfH;
    pointer.wz = 0;
  }

  function applyTheme() {
    theme = darkMq && darkMq.matches ? THEMES.dark : THEMES.light;
  }

  function sync() {
    applyTheme();
    stop();
    if (document.hidden) return;
    if (reduceMq && reduceMq.matches) {
      // Reduced motion: run the simulation forward once (cheap — the sim
      // is one quad pass per step), then hold a single still frame.
      for (let i = 0; i < WARMUP; i++) simStep(1, 4321 / 1000 + i / 60);
      draw(4321);
    } else {
      start();
    }
  }

  function fallback() {
    // No WebGL2 / no float render targets: leave a static CSS fog in the
    // theme's blue family and keep the wordmark. The console is untouched.
    gl = null;
    stop();
    layer.style.backgroundImage =
      "radial-gradient(420px 320px at 18% 30%, rgba(160,170,175,0.10), transparent 70%)," +
      "radial-gradient(520px 380px at 82% 72%, rgba(120,130,135,0.08), transparent 70%)," +
      "radial-gradient(360px 300px at 62% 18%, rgba(54,224,160,0.06), transparent 70%)";
  }

  try {
    initGL();
    resize();
    // Warm the clouds so the first paint already shows an evolved sky.
    for (let i = 0; i < WARMUP; i++) simStep(1, i / 60);
    if (darkMq && darkMq.addEventListener) darkMq.addEventListener("change", sync);
    if (reduceMq && reduceMq.addEventListener) reduceMq.addEventListener("change", sync);
    document.addEventListener("visibilitychange", sync);
    window.addEventListener("resize", () => {
      resize();
      sync();
    });
    window.addEventListener("pointermove", (e) => {
      pointer.x = e.clientX;
      pointer.y = e.clientY;
      pointer.nx = (e.clientX / window.innerWidth) * 2 - 1;
      pointer.ny = -((e.clientY / window.innerHeight) * 2 - 1);
      pointer.on = true;
      updateMouseWorld();
      // Wordmark parallax shares the same pointer offsets (CSS turns them
      // into a few px of translate) — one depth cue across both strata.
      layer.style.setProperty("--pmx", (pointer.nx / 2).toFixed(3));
      layer.style.setProperty("--pmy", (-pointer.ny / 2).toFixed(3));
    });
    document.documentElement.addEventListener("pointerleave", () => {
      pointer.on = false;
      layer.style.setProperty("--pmx", "0");
      layer.style.setProperty("--pmy", "0");
    });
    window.addEventListener("blur", () => {
      pointer.on = false;
    });
    sync();
  } catch (e) {
    fallback();
  }
})();
