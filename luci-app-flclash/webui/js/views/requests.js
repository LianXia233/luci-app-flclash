/* View: 连接 — live connections table with filter, pause and per-row close. */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function requests(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';
    let paused = false;
    let keyword = '';

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="search">' + icon('search', 16) + '<input class="input" type="search" placeholder="筛选主机或规则" aria-label="筛选连接"></div>' +
        '<button class="btn" data-act="pause" type="button">' + icon('pause', 16) + '暂停</button>' +
        '<button class="btn is-ghost" data-act="close-all" type="button">' + icon('trash', 16) + '清空</button>' +
      '</div>' +
      '<section class="card is-flush">' +
        '<div class="card-head"><span class="glyph" data-icon="requests"></span><span class="card-title">实时连接</span>' +
        '<span class="card-end"><span class="chip is-primary" data-role="count">0</span></span></div>' +
        '<div class="table-wrap" data-role="table"></div>' +
      '</section>';

    const tableWrap = ui.$('[data-role="table"]', root);
    const countChip = ui.$('[data-role="count"]', root);
    const pauseBtn = ui.$('[data-act="pause"]', root);

    function rows() {
      const list = (ctx.state.connections || []).filter(function (item) {
        if (!keyword) return true;
        return (item.host + ' ' + item.rule + ' ' + item.chain).toLowerCase().indexOf(keyword) !== -1;
      });
      return list.slice().sort(function (a, b) { return (b.down || 0) - (a.down || 0); });
    }

    function render() {
      const data = rows();
      countChip.textContent = data.length;
      if (!data.length) {
        tableWrap.innerHTML = ui.empty(paused ? '已暂停' : '暂无连接', 'requests');
        return;
      }
      tableWrap.innerHTML = '<table class="data"><thead><tr>' +
        '<th>目标</th><th>规则</th><th>链路</th><th class="num">上传</th><th class="num">下载</th><th>时长</th><th></th>' +
        '</tr></thead><tbody>' + data.slice(0, 100).map(function (item) {
          return '<tr>' +
            '<td title="' + ui.esc(item.host) + '">' + ui.esc(item.host) + ':' + ui.esc(item.port) +
              '<div class="muted" style="font-size:11px">' + ui.esc(item.source || '') + '</div></td>' +
            '<td class="is-muted">' + ui.esc(item.rule || '-') + '</td>' +
            '<td>' + ui.esc(item.chain || '-') + '</td>' +
            '<td class="num">' + ui.bytes(item.up) + '</td>' +
            '<td class="num">' + ui.bytes(item.down) + '</td>' +
            '<td class="is-muted">' + ui.duration(item.duration) + '</td>' +
            '<td><button class="icon-button" data-close="' + ui.esc(item.id) + '" type="button" aria-label="关闭连接">' + icon('close', 14) + '</button></td>' +
            '</tr>';
        }).join('') + '</tbody></table>';

      ui.$$('[data-close]', tableWrap).forEach(function (button) {
        button.addEventListener('click', function () {
          api.closeConnection(button.getAttribute('data-close'))
            .then(function () { return ctx.reloadConnections(); })
            .catch(function (error) { ui.toast('关闭失败: ' + error.message, 'error'); });
        });
      });
    }

    ui.$('input', root).addEventListener('input', ui.debounce(function (event) {
      keyword = event.target.value.trim();
      render();
    }, 150));

    pauseBtn.addEventListener('click', function () {
      paused = !paused;
      pauseBtn.classList.toggle('is-primary', paused);
      pauseBtn.innerHTML = icon(paused ? 'play' : 'pause', 16) + (paused ? '继续' : '暂停');
      if (!paused) render();
    });

    ui.$('[data-act="close-all"]', root).addEventListener('click', function () {
      const list = (ctx.state.connections || []).slice();
      if (!list.length) { ui.toast('当前没有连接'); return; }
      ui.confirmSheet('关闭所有连接', '确定关闭当前 ' + list.length + ' 条连接吗?', function () {
        const button = ui.$('[data-act="close-all"]', root);
        button.disabled = true;
        Promise.all(list.map(function (item) {
          return api.closeConnection(item.id).catch(function () { return null; });
        })).then(function () {
          ctx.state.connections = [];
          render();
          ui.toast('已关闭 ' + list.length + ' 条连接', 'ok');
        }).catch(function (error) {
          ui.toast('关闭失败: ' + error.message, 'error');
        }).then(function () { button.disabled = false; });
      });
    });

    render();
    ctx.onConnections = render;
    ui.every('connections', function () {
      api.connections().then(function (list) {
        ctx.state.connections = list;
        render();
      }).catch(function () {});
    }, 3000);
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.requests = requests;
})();
