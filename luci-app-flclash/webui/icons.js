/* FlClash WebUI — icons via the Material Symbols variable font, the same
 * icon system the Flutter client renders. Icon "drawing" is done by the
 * font's ligatures: an icon is simply the icon name as text, so what shows
 * here matches the upstream app 1:1. `icon(name, size)` keeps the legacy
 * string API used across the views. */
window.FlClashIcons = (function () {
  'use strict';

  /* name → Material Symbols ligature name (upstream client equivalents). */
  const MAP = {
    dashboard: 'dashboard',
    proxies: 'language',
    profiles: 'description',
    requests: 'schedule',
    tools: 'handyman',
    edit: 'edit',
    check: 'check_circle',
    pause: 'pause',
    play: 'play_arrow',
    stop: 'stop_circle',
    arrowUp: 'north',
    arrowDown: 'south',
    shuffle: 'shuffle',
    shield: 'security',
    verified: 'verified_user',
    copy: 'content_copy',
    devices: 'devices',
    memory: 'memory',
    layers: 'layers',
    bolt: 'bolt',
    swap: 'swap_vert',
    search: 'search',
    close: 'close',
    chevron: 'chevron_right',
    plus: 'add',
    trash: 'delete',
    refresh: 'autorenew',
    sync: 'sync',
    dns: 'dns',
    settings: 'settings',
    info: 'info',
    down: 'download',
    split: 'alt_route',
    target: 'adjust',
    status: 'monitor_heart',
    speed: 'speed',
    theme: 'dark_mode',
    sun: 'light_mode',
    menu: 'menu',
    sidebar: 'left_panel_close',
    file: 'description',
    history: 'history',
    key: 'vpn_key',
    globe: 'public',
    wifi: 'wifi',
    language: 'translate',
    palette: 'palette',
    update: 'system_update_alt'
  };

  function svg(name, size) {
    const s = size || 24;
    return '<span class="ms" style="font-size:' + s + 'px;width:' + s + 'px;height:' + s + 'px">' +
      (MAP[name] || 'help') + '</span>';
  }

  function mount(root) {
    (root || document).querySelectorAll('[data-icon]').forEach(function (node) {
      const name = node.getAttribute('data-icon');
      const size = node.getAttribute('data-icon-size');
      node.innerHTML = svg(name, size);
    });
  }

  /* Flower/rosette blob used by the big pause button and country badge. */
  function flower(size, cls) {
    const s = size || 96;
    return '<svg class="' + (cls || '') + '" viewBox="0 0 100 100" width="' + s + '" height="' + s + '" aria-hidden="true">' +
      '<path fill="currentColor" d="M50 2c5 0 9 3 12 7 4-2 9-2 13 1s6 8 5 12c5 1 9 4 10 9s-1 10-4 13c3 3 5 8 4 13s-5 8-10 9c1 5-1 10-5 13s-9 3-13 1c-3 4-7 7-12 7s-9-3-12-7c-4 2-9 2-13-1s-6-8-5-13c-5-1-9-4-10-9s1-10 4-13c-3-3-5-8-4-13s5-8 10-9c-1-5 1-10 5-12s9-3 13-1c3-4 7-7 12-7Z"/></svg>';
  }

  return { svg: svg, mount: mount, flower: flower, map: MAP };
})();
