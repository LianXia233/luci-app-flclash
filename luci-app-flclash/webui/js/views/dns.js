/* View: DNS 测试 — real resolution through the core (GET /dns/query),
 * replacing the old dead "query history" page: mihomo keeps no history, but
 * it does resolve — so the page is an actual working tool. */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function dns(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';
    const history = [];

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="search grow">' + icon('globe', 16) +
          '<input class="input" id="dns-name" type="text" placeholder="输入域名,例如 www.apple.com" aria-label="解析域名">' +
        '</div>' +
        '<select class="select" id="dns-type" aria-label="记录类型">' +
          '<option value="A">A</option><option value="AAAA">AAAA</option><option value="CNAME">CNAME</option>' +
          '<option value="TXT">TXT</option><option value="MX">MX</option><option value="NS">NS</option></select>' +
        '<button class="btn is-primary" data-act="resolve" type="button">' + icon('bolt', 16) + '解析</button>' +
      '</div>' +
      '<section class="card"><div class="card-head"><span class="glyph" data-icon="dns"></span>' +
        '<span class="card-title">解析结果</span><span class="card-end"><span class="chip" data-role="status">就绪</span></span></div>' +
        '<div data-role="result">' + ui.empty('输入域名开始解析测试', 'dns') + '</div></section>' +
      '<section class="card is-flush" style="margin-top:14px">' +
        '<div class="card-head"><span class="glyph" data-icon="history"></span><span class="card-title">本次会话记录</span>' +
        '<span class="card-end"><button class="icon-button" data-act="clear" type="button" aria-label="清空记录">' + icon('trash', 15) + '</button></span></div>' +
        '<div class="dns-history" data-role="history">' + ui.empty('暂无记录', 'history') + '</div>' +
      '</section>';

    const resultBox = ui.$('[data-role="result"]', root);
    const historyBox = ui.$('[data-role="history"]', root);
    const statusChip = ui.$('[data-role="status"]', root);

    function renderHistory() {
      if (!history.length) {
        historyBox.innerHTML = ui.empty('暂无记录', 'history');
        return;
      }
      historyBox.innerHTML = history.map(function (item) {
        return '<div class="dns-row"><span class="dns-time">' + ui.esc(ui.clock(item.time)) + '</span>' +
          '<span class="dns-name">' + ui.esc(item.name) + '</span>' +
          '<span class="chip">' + ui.esc(item.type) + '</span>' +
          '<span class="dns-answer' + (item.fake ? ' is-fake' : '') + '">' + ui.esc(item.answer) + '</span>' +
          '<span class="dns-elapsed">' + ui.esc(item.elapsed) + '</span></div>';
      }).join('');
    }

    function resolve() {
      const name = ui.$('#dns-name', root).value.trim();
      const type = ui.$('#dns-type', root).value;
      if (!name) { ui.toast('请输入域名', 'error'); return; }
      statusChip.textContent = '解析中…';
      statusChip.className = 'chip';
      const started = performance.now();
      api.dnsResolve(name, type)
        .then(function (data) {
          const elapsed = Math.round(performance.now() - started) + ' ms';
          const answers = [];
          (data && data.answer ? data.answer : []).forEach(function (item) {
            if (item && item.data) answers.push(String(item.data));
          });
          const answer = answers.length ? answers.join(', ') : '无记录';
          const fake = answer.indexOf('198.18.') === 0 || answer.indexOf('28.0.0.') === 0;
          statusChip.textContent = answers.length ? '成功' : '空';
          statusChip.className = 'chip' + (answers.length ? ' is-ok' : ' is-warn');
          resultBox.innerHTML = '<div class="kv"><span class="k">域名</span><span class="v">' + ui.esc(name) + '</span></div>' +
            '<div class="kv"><span class="k">类型</span><span class="v">' + ui.esc(type) + '</span></div>' +
            '<div class="kv"><span class="k">结果</span><span class="v dns-answer' + (fake ? ' is-fake' : '') + '">' + ui.esc(answer) + '</span></div>' +
            '<div class="kv"><span class="k">耗时</span><span class="v">' + ui.esc(elapsed) + '</span></div>' +
            (fake ? '<p class="sheet-note">结果为 fake-ip(198.18.0.0/15),属内核正常劫持行为。</p>' : '');
          history.unshift({ time: Math.floor(Date.now() / 1000), name: name, type: type, answer: answer, fake: fake, elapsed: elapsed });
          if (history.length > 20) history.pop();
          renderHistory();
        })
        .catch(function (error) {
          statusChip.textContent = '失败';
          statusChip.className = 'chip is-warn';
          resultBox.innerHTML = ui.empty(error.message, 'info');
        });
    }

    ui.$('[data-act="resolve"]', root).addEventListener('click', resolve);
    ui.$('#dns-name', root).addEventListener('keydown', function (event) {
      if (event.key === 'Enter') resolve();
    });
    ui.$('[data-act="clear"]', root).addEventListener('click', function () {
      history.length = 0;
      renderHistory();
    });

    renderHistory();
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.dns = dns;
})();
