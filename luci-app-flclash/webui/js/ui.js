/* FlClash WebUI — shared UI primitives: dom helpers, toasts, sheets,
 * switches, charts, counters, press ripple. View files stay lean by
 * consuming these. */
window.FlClashUI = (function () {
  'use strict';

  function $(selector, root) {
    return (root || document).querySelector(selector);
  }

  function $$(selector, root) {
    return Array.prototype.slice.call((root || document).querySelectorAll(selector));
  }

  function esc(text) {
    return String(text == null ? '' : text)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  /* ------------------------------------------------------------ formatting */

  function bytes(value) {
    const v = Number(value) || 0;
    if (v < 1024) return Math.round(v) + ' B';
    const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
    let n = v, i = -1;
    do { n /= 1024; i++; } while (n >= 1024 && i < units.length - 1);
    return (n >= 100 ? n.toFixed(0) : n.toFixed(1)) + ' ' + units[i];
  }

  function splitBytes(value) {
    const v = Number(value) || 0;
    if (v < 1024) return { value: String(Math.round(v)), unit: 'B' };
    const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
    let n = v, i = -1;
    do { n /= 1024; i++; } while (n >= 1024 && i < units.length - 1);
    return { value: n >= 100 ? n.toFixed(0) : n.toFixed(1), unit: units[i] };
  }

  function rate(value) {
    const split = splitBytes(value);
    return split.value + ' ' + split.unit + '/s';
  }

  function count(value) {
    const v = Number(value) || 0;
    if (v >= 10000) return (v / 10000).toFixed(1).replace(/\.0$/, '') + 'w';
    if (v >= 1000) return (v / 1000).toFixed(1).replace(/\.0$/, '') + 'k';
    return String(v);
  }

  function duration(seconds) {
    const s = Math.max(0, Math.floor(Number(seconds) || 0));
    const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
    const pad = function (x) { return String(x).padStart(2, '0'); };
    if (h > 24) {
      const days = Math.floor(h / 24);
      return days + ' 天 ' + pad(h % 24) + ':' + pad(m) + ':' + pad(sec);
    }
    return pad(h) + ':' + pad(m) + ':' + pad(sec);
  }

  function clock(ts) {
    const d = new Date((Number(ts) || 0) * 1000);
    const pad = function (x) { return String(x).padStart(2, '0'); };
    return pad(d.getHours()) + ':' + pad(d.getMinutes()) + ':' + pad(d.getSeconds());
  }

  function delayTone(delay) {
    const v = Number(delay) || 0;
    if (!v) return 'bad';
    if (v < 200) return 'ok';
    if (v < 500) return 'warn';
    return 'bad';
  }

  /* --------------------------------------------------------------- polling */

  const timers = {};

  function every(key, fn, interval) {
    stop(key);
    timers[key] = window.setInterval(fn, interval || 2000);
    fn();
  }

  function stop(key) {
    if (timers[key]) { window.clearInterval(timers[key]); delete timers[key]; }
  }

  function stopAll() {
    Object.keys(timers).forEach(stop);
  }

  function debounce(fn, ms) {
    let timer = null;
    return function () {
      const args = arguments, self = this;
      window.clearTimeout(timer);
      timer = window.setTimeout(function () { fn.apply(self, args); }, ms || 150);
    };
  }

  /* ----------------------------------------------------------------- toast */

  function toast(message, kind) {
    const stack = document.getElementById('toast-stack');
    if (!stack) return;
    const node = document.createElement('div');
    node.className = 'toast ' + (kind === 'error' ? 'is-error' : kind === 'ok' ? 'is-ok' : '');
    node.innerHTML = '<span class="toast-dot"></span><span>' + esc(message) + '</span>';
    stack.appendChild(node);
    window.requestAnimationFrame(function () { node.classList.add('is-in'); });
    window.setTimeout(function () {
      node.classList.remove('is-in');
      window.setTimeout(function () { node.remove(); }, 240);
    }, 3200);
  }

  /* ----------------------------------------------------------------- sheet */

  /* Responsive config dialog: right drawer on desktop, bottom sheet on
   * mobile. Dismissal rules are strict on purpose:
   *   - clicking the backdrop NEVER closes (no accidental config loss)
   *   - closing goes only through the explicit close button (or Esc)
   *   - with unsaved edits the close request is intercepted and an inline
   *     bar offers 继续编辑 / 放弃更改 */

  let sheetDirty = false;
  let sheetBusy = false;

  function isMobileViewport() { return window.innerWidth < 860; }

  function openSheet(title, html, bind, opts) {
    const options = opts || {};
    const backdrop = document.getElementById('sheet-backdrop');
    const sheet = document.getElementById('sheet');
    if (!sheet) return;
    sheetDirty = false;
    sheetBusy = typeof options.busy === 'function' ? options.busy : null;

    document.getElementById('sheet-title').textContent = title;
    const body = document.getElementById('sheet-body');
    body.innerHTML = html;
    window.FlClashIcons.mount(body);

    document.documentElement.classList.toggle('is-mobile-sheet', isMobileViewport());
    backdrop.hidden = false;
    sheet.setAttribute('aria-hidden', 'false');
    window.requestAnimationFrame(function () {
      backdrop.classList.add('is-in');
      sheet.classList.add('is-in');
    });

    document.getElementById('sheet-close').onclick = function () { requestSheetClose(); };
    /* Deliberately NOT closing on backdrop/outside click. */
    backdrop.onclick = null;

    body.addEventListener('input', function () { sheetDirty = true; }, true);
    body.addEventListener('change', function () { sheetDirty = true; }, true);
    removeUnsavedBar(body);

    if (typeof bind === 'function') bind(body, requestSheetClose, {
      markSaved: function () { sheetDirty = false; removeUnsavedBar(body); }
    });
  }

  function removeUnsavedBar(body) {
    const bar = $('.unsaved-bar', body);
    if (bar) bar.remove();
  }

  function requestSheetClose() {
    const body = document.getElementById('sheet-body');
    if (sheetBusy && sheetBusy()) return; /* async work in flight */
    if (sheetDirty) { showUnsavedBar(body); return; }
    closeSheet();
  }

  function showUnsavedBar(body) {
    let bar = $('.unsaved-bar', body);
    if (bar) { bar.classList.remove('is-shown'); void bar.offsetWidth; bar.classList.add('is-shown'); return; }
    bar = document.createElement('div');
    bar.className = 'unsaved-bar is-shown';
    bar.innerHTML = '<span>有未保存的更改</span>' +
      '<button class="btn" data-keep type="button">继续编辑</button>' +
      '<button class="btn is-danger" data-discard type="button">放弃更改</button>';
    body.insertBefore(bar, body.firstChild);
    $('[data-keep]', bar).addEventListener('click', function () {
      sheetDirty = false;
      bar.remove();
    });
    $('[data-discard]', bar).addEventListener('click', function () {
      sheetDirty = false;
      closeSheet();
    });
  }

  function closeSheet() {
    const backdrop = document.getElementById('sheet-backdrop');
    const sheet = document.getElementById('sheet');
    if (!sheet || sheet.getAttribute('aria-hidden') === 'true') return;
    backdrop.classList.remove('is-in');
    sheet.classList.remove('is-in');
    sheet.setAttribute('aria-hidden', 'true');
    sheetDirty = false;
    window.setTimeout(function () { backdrop.hidden = true; }, 260);
  }

  document.addEventListener('keydown', function (event) {
    if (event.key !== 'Escape') return;
    const sheet = document.getElementById('sheet');
    if (sheet && sheet.getAttribute('aria-hidden') === 'false') requestSheetClose();
  });

  function confirmSheet(title, text, onConfirm, danger) {
    openSheet(title,
      '<p class="sheet-note">' + esc(text) + '</p>' +
      '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
      '<button class="btn ' + (danger === false ? 'is-primary' : 'is-danger') + '" data-confirm type="button">确定</button></div>',
      function (body, close) {
        $('[data-cancel]', body).addEventListener('click', close);
        $('[data-confirm]', body).addEventListener('click', function () {
          close();
          onConfirm();
        });
      });
  }

  /* --------------------------------------------------------------- widgets */

  function switchHtml(key, on, label) {
    return '<button class="switch' + (on ? ' is-on' : '') + '" role="switch" aria-checked="' +
      (!!on) + '" aria-label="' + esc(label || key) + '" data-switch="' + esc(key) + '" type="button">' +
      '<span class="thumb"></span></button>';
  }

  function head(title, opts) {
    const options = opts || {};
    return '<div class="card-head"><span class="glyph" data-icon="' + options.glyph + '"></span>' +
      '<span class="card-title">' + esc(title) + '</span>' +
      (options.end ? '<span class="card-end">' + options.end + '</span>' : '') +
      (options.chevron ? '<span class="chevron" data-icon="chevron"></span>' : '') +
      '</div>';
  }

  function empty(text, glyph) {
    return '<div class="empty"><span class="empty-icon" data-icon="' + (glyph || 'info') + '"></span>' +
      '<span>' + esc(text) + '</span></div>';
  }

  function skeletonRows(count) {
    let rows = '';
    for (let i = 0; i < (count || 3); i++) {
      rows += '<div class="sk-row" style="animation-delay:' + (i * 0.12) + 's"></div>';
    }
    return '<div class="skeleton">' + rows + '</div>';
  }

  /* Press feedback: a Material-style ripple + pressed scale. */
  function pressable(node, handler) {
    node.classList.add('pressable');
    node.addEventListener('pointerdown', function (event) {
      const rect = node.getBoundingClientRect();
      const ripple = document.createElement('span');
      ripple.className = 'ripple';
      const size = Math.max(rect.width, rect.height);
      ripple.style.width = ripple.style.height = size + 'px';
      ripple.style.left = (event.clientX - rect.left - size / 2) + 'px';
      ripple.style.top = (event.clientY - rect.top - size / 2) + 'px';
      node.appendChild(ripple);
      window.setTimeout(function () { ripple.remove(); }, 520);
    });
    node.addEventListener('click', function (event) { handler(event); });
  }

  function countUp(node, target, format) {
    const from = Number(node.dataset.value || 0);
    const to = Number(target) || 0;
    if (Math.abs(to - from) < 0.01) {
      node.textContent = format ? format(to) : String(to);
      node.dataset.value = String(to);
      return;
    }
    const start = performance.now();
    const step = function (now) {
      const t = Math.min(1, (now - start) / 360);
      const eased = 1 - Math.pow(1 - t, 3);
      const value = from + (to - from) * eased;
      node.textContent = format ? format(value) : String(Math.round(value));
      if (t < 1) window.requestAnimationFrame(step);
      else node.dataset.value = String(to);
    };
    window.requestAnimationFrame(step);
  }

  /* Smooth sparkline with area fill; animates on each redraw. */
  function renderChart(node, values, opts) {
    const options = opts || {};
    const height = options.height || 64;
    const width = node.clientWidth || 300;
    const data = (values || []).slice(-60);
    if (!data.length) { node.innerHTML = ''; return; }
    const max = Math.max.apply(null, data.concat([options.minScale || 1024])) * 1.15;
    const stepX = data.length > 1 ? width / (data.length - 1) : width;
    const points = data.map(function (v, i) {
      const x = i * stepX;
      const y = height - (v / max) * (height - 6) - 3;
      return [x.toFixed(1), y.toFixed(1)];
    });
    let path = 'M' + points[0][0] + ' ' + points[0][1];
    for (let i = 1; i < points.length; i++) {
      const prev = points[i - 1], cur = points[i];
      const mx = ((Number(prev[0]) + Number(cur[0])) / 2).toFixed(1);
      path += ' C' + mx + ' ' + prev[1] + ' ' + mx + ' ' + cur[1] + ' ' + cur[0] + ' ' + cur[1];
    }
    const area = path + ' L' + width + ' ' + height + ' L0 ' + height + ' Z';
    const tone = options.tone || (node.classList.contains('is-up') ? 'up' : node.classList.contains('is-down') ? 'down' : 'up');
    node.innerHTML =
      '<svg viewBox="0 0 ' + width + ' ' + height + '" preserveAspectRatio="none" class="chart-svg">' +
      '<defs><linearGradient id="grad-' + tone + '-' + Math.round(Math.random() * 1e6) + '" x1="0" y1="0" x2="0" y2="1">' +
      '<stop offset="0" stop-color="var(--chart-' + tone + ')" stop-opacity="0.35"/>' +
      '<stop offset="1" stop-color="var(--chart-' + tone + ')" stop-opacity="0"/></linearGradient></defs>' +
      '<path class="chart-area" d="' + area + '" fill="url(#grad-' + tone + '-' + Math.round(Math.random() * 1e6) + ')"/>' +
      '<path class="chart-line" d="' + path + '" stroke="var(--chart-' + tone + ')" stroke-width="2" fill="none" stroke-linecap="round"/>' +
      '</svg>';
  }

  /* Wave usage bar (profile cards). */
  function wave(share, width, height) {
    const w = width || 300, h = height || 12;
    const p = Math.max(0, Math.min(100, Math.round((Number(share) || 0) * 100)));
    return '<rect x="0" y="' + ((h - 4) / 2) + '" width="' + w + '" height="4" rx="2" class="wave-track"/>' +
      '<rect x="0" y="' + ((h - 4) / 2) + '" width="' + Math.max(6, w * p / 100) + '" height="4" rx="2" class="wave-fill"/>';
  }

  function letterTile(name, cls) {
    const text = String(name || '?').trim();
    const first = text ? text.charAt(0).toUpperCase() : '?';
    return '<span class="letter-tile ' + (cls || '') + '">' + esc(first) + '</span>';
  }

  return {
    $: $, $$: $$, esc: esc, bytes: bytes, splitBytes: splitBytes, rate: rate,
    count: count, duration: duration, clock: clock, delayTone: delayTone,
    every: every, stop: stop, stopAll: stopAll, debounce: debounce,
    toast: toast, openSheet: openSheet, closeSheet: closeSheet, confirmSheet: confirmSheet,
    switchHtml: switchHtml, head: head, empty: empty, skeletonRows: skeletonRows,
    pressable: pressable, countUp: countUp, renderChart: renderChart, wave: wave,
    letterTile: letterTile
  };
})();
