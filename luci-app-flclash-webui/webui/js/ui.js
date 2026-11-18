/* Shared UI primitives: formatting, toasts, sheets, charts, polling. */
window.FlClashUI = (function () {
  'use strict';

  const KB = 1024;
  const MB = KB * 1024;
  const GB = MB * 1024;
  const TB = GB * 1024;

  function $(sel, root) { return (root || document).querySelector(sel); }
  function $$(sel, root) { return Array.prototype.slice.call((root || document).querySelectorAll(sel)); }

  function esc(value) {
    return String(value == null ? '' : value)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  function icon(name, cls) { return window.FlClashIcons.svg(name, cls); }

  function glyph(name, cls) {
    return '<span class="glyph' + (cls ? ' ' + cls : '') + '">' + icon(name) + '</span>';
  }

  function splitBytes(bytes) {
    const value = Math.max(0, Number(bytes) || 0);
    if (value < KB) return { value: String(Math.round(value)), unit: 'B' };
    if (value < MB) return { value: (value / KB).toFixed(1), unit: 'KB' };
    if (value < GB) return { value: (value / MB).toFixed(1), unit: 'MB' };
    if (value < TB) return { value: (value / GB).toFixed(2), unit: 'GB' };
    return { value: (value / TB).toFixed(2), unit: 'TB' };
  }

  function bytes(value) {
    const part = splitBytes(value);
    return part.value + ' ' + part.unit;
  }

  function rate(value) {
    const part = splitBytes(value);
    return part.value + ' ' + part.unit + '/s';
  }

  function count(value) {
    const n = Number(value) || 0;
    if (n < 100000) return n.toLocaleString('en-US');
    if (n < 1000000) return (n / 1000).toFixed(1).replace(/\.0$/, '') + 'K';
    return (n / 1000000).toFixed(1).replace(/\.0$/, '') + 'M';
  }

  function duration(seconds) {
    const total = Math.max(0, Math.floor(Number(seconds) || 0));
    const d = Math.floor(total / 86400);
    const h = Math.floor((total % 86400) / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = total % 60;
    const pad = (v) => String(v).padStart(2, '0');
    if (d > 0) return d + '天 ' + pad(h) + ':' + pad(m) + ':' + pad(s);
    return pad(h) + ':' + pad(m) + ':' + pad(s);
  }

  function clock(value) {
    const date = value instanceof Date ? value : new Date(value || Date.now());
    if (isNaN(date.getTime())) return String(value || '');
    return date.toLocaleTimeString('zh-CN', { hour12: false });
  }

  function dayTime(value) {
    const date = value instanceof Date ? value : new Date(value);
    if (isNaN(date.getTime())) return String(value || '');
    return (date.getMonth() + 1) + '/' + date.getDate() + ' ' + clock(date);
  }

  function delayTone(delay) {
    const n = Number(delay) || 0;
    if (!n) return 'bad';
    if (n < 200) return 'ok';
    if (n < 500) return 'warn';
    return 'bad';
  }

  const toastStack = document.getElementById('toast-stack');

  function toast(message, type) {
    if (!toastStack) return;
    const node = document.createElement('div');
    node.className = 'toast' + (type ? ' is-' + type : '');
    node.textContent = message;
    toastStack.appendChild(node);
    const life = type === 'error' ? 3600 : 2400;
    window.setTimeout(function () {
      node.classList.add('is-out');
      window.setTimeout(function () { node.remove(); }, 220);
    }, life);
  }

  const sheet = document.getElementById('sheet');
  const backdrop = document.getElementById('sheet-backdrop');
  const sheetTitle = document.getElementById('sheet-title');
  const sheetBody = document.getElementById('sheet-body');
  let lastFocus = null;

  function openSheet(title, html, onMount) {
    if (!sheet) return;
    lastFocus = document.activeElement;
    sheetTitle.textContent = title;
    sheetBody.innerHTML = html;
    backdrop.hidden = false;
    window.requestAnimationFrame(function () {
      backdrop.classList.add('is-open');
      sheet.classList.add('is-open');
    });
    sheet.setAttribute('aria-hidden', 'false');
    window.FlClashIcons.mount(sheet);
    if (onMount) onMount(sheetBody);
    const focusable = sheet.querySelector('button, input, select, textarea');
    if (focusable) window.setTimeout(function () { focusable.focus(); }, 120);
  }

  function closeSheet() {
    if (!sheet) return;
    sheet.classList.remove('is-open');
    backdrop.classList.remove('is-open');
    sheet.setAttribute('aria-hidden', 'true');
    window.setTimeout(function () {
      backdrop.hidden = true;
      sheetBody.innerHTML = '';
    }, 320);
    if (lastFocus && lastFocus.focus) lastFocus.focus();
  }

  /** Press feedback shared by every tappable surface. */
  function pressable(node, onActivate) {
    let pressed = false;
    const down = function () {
      pressed = true;
      node.classList.add('is-pressed');
    };
    const up = function () {
      if (!pressed) return;
      pressed = false;
      node.classList.remove('is-pressed');
    };
    node.addEventListener('pointerdown', down);
    node.addEventListener('pointerup', up);
    node.addEventListener('pointerleave', up);
    node.addEventListener('pointercancel', up);
    node.addEventListener('click', function (event) {
      if (node.classList.contains('is-disabled')) return;
      if (onActivate) onActivate(event);
    });
    node.addEventListener('keydown', function (event) {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        if (onActivate) onActivate(event);
      }
    });
    if (!node.hasAttribute('tabindex') && node.tagName !== 'BUTTON') node.setAttribute('tabindex', '0');
  }

  const series = new WeakMap();

  function smoothPath(points) {
    if (points.length < 2) return points.length ? 'M' + points[0][0] + ' ' + points[0][1] : '';
    let d = 'M' + points[0][0].toFixed(2) + ' ' + points[0][1].toFixed(2);
    for (let i = 0; i < points.length - 1; i += 1) {
      const p0 = points[i - 1] || points[i];
      const p1 = points[i];
      const p2 = points[i + 1];
      const p3 = points[i + 2] || p2;
      const t = 0.18;
      const c1x = p1[0] + (p2[0] - p0[0]) * t;
      const c1y = p1[1] + (p2[1] - p0[1]) * t;
      const c2x = p2[0] - (p3[0] - p1[0]) * t;
      const c2y = p2[1] - (p3[1] - p1[1]) * t;
      d += ' C' + c1x.toFixed(2) + ' ' + c1y.toFixed(2) + ',' + c2x.toFixed(2) + ' ' + c2y.toFixed(2) +
        ',' + p2[0].toFixed(2) + ' ' + p2[1].toFixed(2);
    }
    return d;
  }

  /**
   * Traffic chart: values are bytes/s, baseline at the bottom, minimum scale
   * 8 KiB/s like the dashboard's line chart.
   */
  function renderChart(container, values, options) {
    const opts = options || {};
    const width = 300;
    const height = opts.height || 88;
    const data = (values && values.length ? values : [0, 0]).map(function (v) { return Math.max(0, Number(v) || 0); });
    let max = Math.max.apply(null, data);
    max = Math.max(max, opts.minScale || 8192);
    const points = data.map(function (v, i) {
      const x = (i / Math.max(1, data.length - 1)) * width;
      const y = height - 2 - (v / max) * (height - 8);
      return [x, y];
    });
    const line = smoothPath(points);
    const area = line + ' L' + width + ' ' + height + ' L0 ' + height + ' Z';
    const previous = series.get(container);
    const key = line;
    if (previous !== key) {
      series.set(container, key);
      container.innerHTML =
        '<svg viewBox="0 0 ' + width + ' ' + height + '" preserveAspectRatio="none">' +
        '<path class="area" d="' + area + '"></path>' +
        '<path class="line" d="' + line + '"></path>' +
        '</svg>';
      const path = container.querySelector('path.line');
      if (path && path.getTotalLength) {
        const len = path.getTotalLength();
        path.style.strokeDasharray = len;
        path.style.strokeDashoffset = len;
        path.style.transition = 'stroke-dashoffset 520ms cubic-bezier(0.22,1,0.36,1)';
        window.requestAnimationFrame(function () { path.style.strokeDashoffset = '0'; });
      }
    }
  }

  function sparkline(values, width, height) {
    const data = (values && values.length ? values : [0, 0]);
    const max = Math.max.apply(null, data) || 1;
    const points = data.map(function (v, i) {
      return [(i / Math.max(1, data.length - 1)) * width, height - (v / max) * (height - 4) - 2];
    });
    return smoothPath(points);
  }

  /** Usage wave: a sine track that turns solid up to `share`. */
  function wave(share, width, height) {
    const value = Math.max(0, Math.min(1, Number(share) || 0));
    const mid = height / 2;
    const end = width - 2;
    const split = 2 + (end - 2) * value;
    let solid = 'M2 ' + mid;
    for (let x = 2; x <= split; x += 2) {
      solid += ' L' + x.toFixed(1) + ' ' + (mid + (height / 4) * Math.sin(((x - 2) / 10) * 2 * Math.PI)).toFixed(2);
    }
    let track = '';
    if (split < end - 4) {
      track = 'M' + (split + 6).toFixed(1) + ' ' + mid;
      for (let x = split + 6; x <= end; x += 2) {
        track += ' L' + x.toFixed(1) + ' ' + (mid + (height / 4) * Math.sin(((x - 2) / 10) * 2 * Math.PI)).toFixed(2);
      }
    }
    return '<path d="' + solid + '" fill="none" stroke="var(--primary)" stroke-width="2.6" stroke-linecap="round"/>' +
      (track ? '<path d="' + track + '" fill="none" stroke="var(--track)" stroke-width="2.6" stroke-linecap="round"/>' : '');
  }

  const counters = new WeakMap();

  function countUp(node, target, formatter) {
    if (!node) return;
    const from = counters.get(node) || 0;
    const to = Number(target) || 0;
    counters.set(node, to);
    if (Math.abs(to - from) < 0.001) {
      node.textContent = formatter ? formatter(to) : String(to);
      return;
    }
    const start = performance.now();
    const span = 420;
    function step(now) {
      const t = Math.min(1, (now - start) / span);
      const eased = 1 - Math.pow(1 - t, 3);
      const value = from + (to - from) * eased;
      node.textContent = formatter ? formatter(value) : String(Math.round(value));
      if (t < 1) window.requestAnimationFrame(step);
    }
    window.requestAnimationFrame(step);
  }

  const timers = new Map();

  function every(key, fn, interval) {
    stop(key);
    timers.set(key, window.setInterval(fn, interval));
  }

  function stop(key) {
    if (timers.has(key)) {
      window.clearInterval(timers.get(key));
      timers.delete(key);
    }
  }

  function stopAll() {
    timers.forEach(function (id) { window.clearInterval(id); });
    timers.clear();
  }

  function debounce(fn, wait) {
    let id = 0;
    return function () {
      const args = arguments;
      window.clearTimeout(id);
      id = window.setTimeout(function () { fn.apply(null, args); }, wait || 200);
    };
  }

  function card(className, html, onTap) {
    const node = document.createElement('section');
    node.className = 'card ' + (className || '');
    node.innerHTML = html;
    if (onTap) {
      node.classList.add('is-tappable');
      pressable(node, onTap);
    }
    return node;
  }

  function empty(text, iconName) {
    return '<div class="empty">' + icon(iconName || 'info') + '<span>' + esc(text) + '</span></div>';
  }

  function head(title, options) {
    const opts = options || {};
    return '<div class="card-head">' +
      (opts.glyph ? glyph(opts.glyph) : '') +
      '<span class="card-title">' + esc(title) + '</span>' +
      (opts.leading || '') +
      (opts.end ? '<span class="card-end">' + opts.end + '</span>' : '') +
      (opts.chevron ? '<span class="chevron">' + icon('chevron') + '</span>' : '') +
      '</div>';
  }

  function switchHtml(name, on, label) {
    return '<button class="switch' + (on ? ' is-on' : '') + '" type="button" role="switch" ' +
      'aria-checked="' + (!!on) + '" aria-label="' + esc(label || name) + '" data-switch="' + esc(name) + '"></button>';
  }

  return {
    $: $, $$: $$, esc: esc, icon: icon, glyph: glyph,
    bytes: bytes, rate: rate, splitBytes: splitBytes, count: count,
    duration: duration, clock: clock, dayTime: dayTime, delayTone: delayTone,
    toast: toast, openSheet: openSheet, closeSheet: closeSheet, pressable: pressable,
    renderChart: renderChart, sparkline: sparkline, wave: wave, countUp: countUp,
    every: every, stop: stop, stopAll: stopAll, debounce: debounce,
    card: card, empty: empty, head: head, switchHtml: switchHtml
  };
})();
