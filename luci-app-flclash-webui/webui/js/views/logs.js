/* View: 日志 — live core log with level filter, pause and export. */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function logs(ctx) {
    const root = document.createElement('div');
    root.className = 'view page';
    let level = '';
    let paused = false;

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="seg-chips" data-role="levels"></div>' +
        '<div class="grow"></div>' +
        '<button class="btn" data-act="pause" type="button">' + icon('pause', 16) + '暂停</button>' +
        '<button class="btn is-ghost" data-act="export" type="button">' + icon('down', 16) + '导出</button>' +
      '</div>' +
      '<div class="log-list" data-role="list"></div>';

    const list = ui.$('[data-role="list"]', root);
    const levels = [['', '全部'], ['info', 'INFO'], ['warning', 'WARN'], ['error', 'ERROR'], ['debug', 'DEBUG']];
    const bar = ui.$('[data-role="levels"]', root);

    function renderLevels() {
      bar.innerHTML = levels.map(function (item) {
        return '<button class="seg-chip' + (level === item[0] ? ' is-active' : '') + '" data-level="' + item[0] + '" type="button">' + item[1] + '</button>';
      }).join('');
      ui.$$('[data-level]', bar).forEach(function (button) {
        button.addEventListener('click', function () {
          level = button.getAttribute('data-level');
          renderLevels();
          render(true);
        });
      });
    }

    function render(force) {
      if (paused && !force) return;
      const data = (ctx.state.logs || []).filter(function (item) {
        if (!level) return true;
        const value = String(item.level || '').toLowerCase();
        if (level === 'warning') return value === 'warning' || value === 'warn';
        return value === level;
      });
      if (!data.length) { list.innerHTML = ui.empty('暂无日志', 'logs'); return; }
      const atBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
      list.innerHTML = data.slice(-300).map(function (item) {
        const value = String(item.level || 'info').toLowerCase();
        return '<div class="log-line"><span class="log-time">' + ui.esc(ui.clock(item.time)) + '</span>' +
          '<span class="log-level ' + ui.esc(value) + '">' + ui.esc(value) + '</span>' +
          '<span class="log-msg">' + ui.esc(item.message) + '</span></div>';
      }).join('');
      if (atBottom || force) list.scrollTop = list.scrollHeight;
    }

    ui.$('[data-act="pause"]', root).addEventListener('click', function () {
      paused = !paused;
      this.classList.toggle('is-primary', paused);
      this.innerHTML = icon(paused ? 'play' : 'pause', 16) + (paused ? '继续' : '暂停');
      if (!paused) render(true);
    });

    ui.$('[data-act="export"]', root).addEventListener('click', function () {
      const text = (ctx.state.logs || []).map(function (item) {
        return new Date((item.time || 0) * 1000).toISOString() + ' [' + item.level + '] ' + item.message;
      }).join('\n');
      const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
      const link = document.createElement('a');
      link.href = URL.createObjectURL(blob);
      link.download = 'flclash-logs.txt';
      link.click();
      window.setTimeout(function () { URL.revokeObjectURL(link.href); }, 2000);
    });

    renderLevels();
    render(true);
    ctx.onLogs = function () { render(false); };
    ui.every('logs', function () {
      api.logs().then(function (data) { ctx.state.logs = data; render(); }).catch(function () {});
    }, 3000);
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.logs = logs;
})();
