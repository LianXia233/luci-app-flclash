/* FlClash glyphs — 24px stroke set, matched to AppGlyphs proportions. */
window.FlClashIcons = (function () {
  'use strict';

  const paths = {
    dashboard: '<rect x="3.5" y="3.5" width="7.2" height="7.2" rx="2.2"/><rect x="13.3" y="3.5" width="7.2" height="7.2" rx="2.2"/><rect x="3.5" y="13.3" width="7.2" height="7.2" rx="2.2"/><rect x="13.3" y="13.3" width="7.2" height="7.2" rx="2.2"/>',
    proxies: '<circle cx="12" cy="12" r="8.6"/><path d="M3.4 12h17.2"/><path d="M12 3.4c2.4 2.6 3.6 5.5 3.6 8.6S14.4 18 12 20.6C9.6 18 8.4 15.1 8.4 12S9.6 6 12 3.4Z"/>',
    profiles: '<path d="M12 3.2 3.6 7.8 12 12.4l8.4-4.6L12 3.2Z"/><path d="m3.6 12 8.4 4.6L20.4 12"/><path d="m3.6 16.2 8.4 4.6 8.4-4.6"/>',
    requests: '<path d="M3.2 12h3.9l2.6-7.1 4.1 14.2 2.7-7.1h4.3"/>',
    dns: '<ellipse cx="12" cy="5.6" rx="7.6" ry="2.9"/><path d="M4.4 5.6v12.8c0 1.6 3.4 2.9 7.6 2.9s7.6-1.3 7.6-2.9V5.6"/><path d="M4.4 12c0 1.6 3.4 2.9 7.6 2.9s7.6-1.3 7.6-2.9"/>',
    logs: '<rect x="3.2" y="4.4" width="17.6" height="15.2" rx="3.2"/><path d="m7.4 9.4 3 2.8-3 2.8"/><path d="M13 15.4h3.6"/>',
    tools: '<rect x="3.2" y="7" width="17.6" height="13" rx="3.2"/><path d="M9 7V5.2a1.6 1.6 0 0 1 1.6-1.6h2.8A1.6 1.6 0 0 1 15 5.2V7"/><path d="M3.2 12.2h17.6"/><path d="M10.2 12.2v2.2h3.6v-2.2"/>',
    status: '<path d="M4.2 12a7.8 7.8 0 1 1 2.3 5.5"/><path d="M4.2 17.2v4.4h4.4"/><path d="M12 8v4.1l2.8 1.7"/>',
    split: '<path d="M5.6 4v4.6a3 3 0 0 0 3 3h9"/><path d="m15 8.8 3 3-3 3"/><path d="M5.6 20.4v-4.6a3 3 0 0 1 3-3h2.2"/>',
    target: '<circle cx="12" cy="12" r="7.8"/><circle cx="12" cy="12" r="3.1"/>',
    shuffle: '<path d="M3.4 7.2h3.8c4.8 0 4.8 9.6 9.6 9.6h3.8"/><path d="m18.2 14.4 2.8 2.4-2.8 2.4"/><path d="M3.4 16.8h3.8c1.9 0 3.1-1.5 4.1-3.2"/><path d="m18.2 4.8 2.8 2.4-2.8 2.4"/>',
    vpn: '<path d="M12 3.4 5.4 6.2v5.1c0 4.2 2.8 7.9 6.6 9.3 3.8-1.4 6.6-5.1 6.6-9.3V6.2L12 3.4Z"/><path d="m9.2 11.8 2 2 3.6-3.8"/>',
    devices: '<rect x="3.2" y="5.2" width="12.4" height="9.4" rx="2.2"/><path d="M6.4 18.8h8"/><rect x="16.4" y="9.2" width="4.4" height="8" rx="1.6"/>',
    verified: '<path d="M12 3.4 5.4 6.2v5.1c0 4.2 2.8 7.9 6.6 9.3 3.8-1.4 6.6-5.1 6.6-9.3V6.2L12 3.4Z"/><path d="m9.2 11.8 2 2 3.6-3.8"/>',
    memory: '<rect x="6.4" y="6.4" width="11.2" height="11.2" rx="2.4"/><path d="M9.6 2.8v3.6M14.4 2.8v3.6M9.6 17.6v3.6M14.4 17.6v3.6M2.8 9.6h3.6M2.8 14.4h3.6M17.6 9.6h3.6M17.6 14.4h3.6"/>',
    speed: '<path d="M4 16.4a8.4 8.4 0 1 1 16 0"/><path d="m12 13.6 3.8-3.8"/><circle cx="12" cy="14.2" r="1.5"/>',
    arrowUp: '<path d="M12 19V5.4"/><path d="m6.4 11 5.6-5.6 5.6 5.6"/>',
    arrowDown: '<path d="M12 5v13.6"/><path d="m6.4 13 5.6 5.6 5.6-5.6"/>',
    play: '<path d="M8.4 5.6 18.6 12 8.4 18.4V5.6Z"/>',
    pause: '<rect x="7.2" y="5.4" width="3.6" height="13.2" rx="1.4"/><rect x="13.2" y="5.4" width="3.6" height="13.2" rx="1.4"/>',
    refresh: '<path d="M20 11.2A8 8 0 1 0 17.7 17"/><path d="M20 4.4v6.8h-6.8"/>',
    check: '<path d="m5.2 12.4 4.4 4.4 9.2-9.6"/>',
    close: '<path d="m6.4 6.4 11.2 11.2M17.6 6.4 6.4 17.6"/>',
    chevron: '<path d="m9.4 6.2 5.8 5.8-5.8 5.8"/>',
    copy: '<rect x="8.2" y="8.2" width="12" height="12" rx="2.4"/><path d="M15.8 8.2V6.4a2.4 2.4 0 0 0-2.4-2.4H6.4A2.4 2.4 0 0 0 4 6.4v7a2.4 2.4 0 0 0 2.4 2.4h1.8"/>',
    plus: '<path d="M12 5.2v13.6M5.2 12h13.6"/>',
    trash: '<path d="M4.4 7.2h15.2"/><path d="M9.4 7.2V4.6h5.2v2.6"/><path d="M6.6 7.2 7.6 20h8.8l1-12.8"/>',
    edit: '<path d="M4 20h4.2L19 9.2 14.8 5 4 15.8V20Z"/><path d="m13.4 6.4 4.2 4.2"/>',
    search: '<circle cx="11" cy="11" r="6.6"/><path d="m16 16 4.2 4.2"/>',
    theme: '<circle cx="12" cy="12" r="4.2"/><path d="M12 2.6v2.2M12 19.2v2.2M4.9 4.9l1.6 1.6M17.5 17.5l1.6 1.6M2.6 12h2.2M19.2 12h2.2M4.9 19.1l1.6-1.6M17.5 6.5l1.6-1.6"/>',
    wifiOff: '<path d="M3.4 8.6a14 14 0 0 1 4.6-2.4M15.4 6.4a14 14 0 0 1 5.2 2.2"/><path d="M6.8 12a9.6 9.6 0 0 1 3.2-1.6M14.8 10.6a9.6 9.6 0 0 1 2.4 1.4"/><path d="M9.8 15.2a5 5 0 0 1 4.4 0"/><circle cx="12" cy="18.6" r="1.1"/><path d="m4 4 16 16"/>',
    logout: '<path d="M9.4 4.6H6.2A2.2 2.2 0 0 0 4 6.8v10.4a2.2 2.2 0 0 0 2.2 2.2h3.2"/><path d="M15.4 8.2 19.2 12l-3.8 3.8"/><path d="M9.6 12h9.6"/>',
    external: '<path d="M13.6 5.2h5.2v5.2"/><path d="m18.8 5.2-7.4 7.4"/><path d="M18.4 14.2v3.4a2.4 2.4 0 0 1-2.4 2.4H6.4A2.4 2.4 0 0 1 4 17.6V8a2.4 2.4 0 0 1 2.4-2.4h3.4"/>',
    bolt: '<path d="M13.4 2.8 5 13.6h6.2l-1 7.6 8.8-11.2h-6.4l.8-7.2Z"/>',
    swap: '<path d="M4.4 8.2h13.2l-3.2-3.2"/><path d="M19.6 15.8H6.4l3.2 3.2"/>',
    info: '<circle cx="12" cy="12" r="8.4"/><path d="M12 11v5.2"/><circle cx="12" cy="8.2" r="1.05"/>',
    clock: '<circle cx="12" cy="12" r="8.4"/><path d="M12 7.4V12l3.2 1.9"/>',
    pauseCircle: '<circle cx="12" cy="12" r="8.6"/><path d="M10 9v6M14 9v6"/>',
    playCircle: '<circle cx="12" cy="12" r="8.6"/><path d="m10.2 8.8 5.2 3.2-5.2 3.2V8.8Z"/>',
    filter: '<path d="M4 6.4h16l-6.2 7.2v5.4l-3.6 1.8v-7.2L4 6.4Z"/>',
    down: '<path d="M12 4.4v11.6"/><path d="m7.6 11.6 4.4 4.4 4.4-4.4"/><path d="M4.8 20h14.4"/>'
  };

  function svg(name, cls) {
    const body = paths[name] || paths.info;
    return '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" ' +
      'stroke-linecap="round" stroke-linejoin="round"' + (cls ? ' class="' + cls + '"' : '') +
      ' aria-hidden="true">' + body + '</svg>';
  }

  function mount(root) {
    (root || document).querySelectorAll('[data-icon]').forEach(function (node) {
      if (node.querySelector('svg')) return;
      node.innerHTML = svg(node.getAttribute('data-icon'));
    });
  }

  return { paths: paths, svg: svg, mount: mount };
})();
