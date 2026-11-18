/* Page views: proxies, profiles, requests, logs, dns, tools, settings. */
window.FlClashViews = (function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = ui.icon;

  function el(html) {
    const node = document.createElement('div');
    node.innerHTML = html.trim();
    return node.firstElementChild;
  }

  function fail(node, error) {
    node.innerHTML = ui.empty(error && error.message ? error.message : '加载失败', 'info');
  }

  /* ------------------------------------------------------------- proxies */

  function proxies(ctx) {
    const api = ctx.api;
    const state = ctx.state;
    const root = el('<div class="view page"></div>');
    let activeGroup = '';
    let keyword = '';
    let sortByDelay = false;

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="search">' + icon('search') + '<input class="input" type="search" placeholder="搜索节点" aria-label="搜索节点"></div>' +
        '<button class="btn" data-act="delay" type="button">' + icon('bolt') + '测速</button>' +
        '<button class="btn is-ghost" data-act="sort" type="button">' + icon('swap') + '按延迟</button>' +
      '</div>' +
      '<div class="seg-chips" data-role="groups"></div>' +
      '<div class="node-grid" data-role="nodes"></div>';

    const search = ui.$('input', root);
    const groupsBar = ui.$('[data-role="groups"]', root);
    const grid = ui.$('[data-role="nodes"]', root);

    function renderGroups() {
      const groups = (state.proxies && state.proxies.groups) || [];
      if (!groups.length) { groupsBar.innerHTML = ''; return; }
      const all = '<button class="seg-chip' + (activeGroup ? '' : ' is-active') + '" data-group="" type="button">全部 ' + groups.reduce(function (n, g) { return n + (g.count || (g.nodes || []).length || 0); }, 0) + '</button>';
      groupsBar.innerHTML = all + groups.map(function (group) {
        return '<button class="seg-chip' + (activeGroup === group.name ? ' is-active' : '') + '" data-group="' + ui.esc(group.name) + '" type="button">' +
          ui.esc(group.name) + ' · ' + ui.esc(group.now || '') + '</button>';
      }).join('');
      ui.$$('[data-group]', groupsBar).forEach(function (button) {
        button.addEventListener('click', function () {
          activeGroup = button.getAttribute('data-group');
          renderGroups();
          renderNodes();
        });
      });
    }

    function nodesOf() {
      const data = state.proxies || { nodes: [], groups: [] };
      let nodes = (data.nodes || []).filter(function (node) {
        if (activeGroup && node.group !== activeGroup) return false;
        if (!keyword) return true;
        return (node.name + ' ' + (node.type || '')).toLowerCase().indexOf(keyword.toLowerCase()) !== -1;
      });
      nodes = nodes.slice().sort(function (a, b) {
        if (sortByDelay) {
          const av = Number(a.delay) || 99999;
          const bv = Number(b.delay) || 99999;
          return av - bv;
        }
        return 0;
      });
      return nodes;
    }

    function nodeCard(node) {
      const delay = Number(node.delay) || 0;
      const tone = ui.delayTone(delay);
      const active = (state.proxies.groups || []).some(function (group) {
        return group.now === node.name && (!activeGroup || group.name === activeGroup);
      });
      return '<div class="node-card' + (active ? ' is-active' : '') + '" data-node="' + ui.esc(node.name) + '" data-group="' + ui.esc(node.group) + '" role="button" tabindex="0">' +
        '<div class="node-top"><span class="node-name">' + ui.esc(node.name) + '</span>' +
          '<span class="chip is-' + tone + '" data-role="delay">' + (delay ? delay + ' ms' : '超时') + '</span></div>' +
        '<div class="node-meta"><span class="chip">' + ui.esc(node.type || 'node') + '</span>' +
          '<span>' + ui.esc(node.group || '') + '</span>' +
          '<button class="btn is-ghost" data-test="' + ui.esc(node.name) + '" type="button" style="height:26px;padding:0 8px;margin-left:auto">测速</button></div>' +
        '</div>';
    }

    function renderNodes() {
      const nodes = nodesOf();
      grid.innerHTML = nodes.length ? nodes.map(nodeCard).join('') : ui.empty('没有匹配的节点', 'proxies');
      ui.$$('[data-test]', grid).forEach(function (button) {
        button.addEventListener('click', function (event) {
          event.stopPropagation();
          button.disabled = true;
          api.testDelay(button.getAttribute('data-test'))
            .then(function (result) { ui.toast(result.name + ' · ' + result.value + ' ms', 'ok'); return ctx.reloadProxies(); })
            .catch(function (error) { ui.toast('测速失败: ' + error.message, 'error'); button.disabled = false; });
        });
      });
      ui.$$('.node-card', grid).forEach(function (node) {
        const activate = function () {
          const group = node.getAttribute('data-group');
          const name = node.getAttribute('data-node');
          api.selectProxy(group, name)
            .then(function () { ui.toast('已切换到 ' + name, 'ok'); return ctx.reloadProxies(); })
            .catch(function (error) { ui.toast('切换失败: ' + error.message, 'error'); });
        };
        ui.pressable(node, activate);
        node.addEventListener('keydown', function (event) {
          if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate(); }
        });
      });
    }

    search.addEventListener('input', ui.debounce(function () {
      keyword = search.value.trim();
      renderNodes();
    }, 150));

    ui.$('[data-act="delay"]', root).addEventListener('click', function () {
      const nodes = nodesOf().slice(0, 24);
      if (!nodes.length) { ui.toast('没有可测速的节点', 'error'); return; }
      ui.toast('开始测速 ' + nodes.length + ' 个节点');
      nodes.forEach(function (node, index) {
        window.setTimeout(function () {
          api.testDelay(node.name).catch(function () {});
          if (index === nodes.length - 1) window.setTimeout(ctx.reloadProxies, 900);
        }, index * 120);
      });
    });

    ui.$('[data-act="sort"]', root).addEventListener('click', function () {
      sortByDelay = !sortByDelay;
      this.classList.toggle('is-primary', sortByDelay);
      renderNodes();
    });

    renderGroups();
    renderNodes();
    ctx.onProxies = function () { renderGroups(); renderNodes(); };
    return root;
  }

  /* ------------------------------------------------------------- profiles */

  function profiles(ctx) {
    const api = ctx.api;
    const root = el('<div class="view page"></div>');
    root.innerHTML =
      '<div class="page-toolbar"><div class="grow muted" data-role="summary">—</div>' +
        '<button class="btn is-primary" data-act="import" type="button">' + icon('plus') + '导入配置</button></div>' +
      '<div class="profile-list" data-role="list"></div>';

    const list = ui.$('[data-role="list"]', root);

    function render() {
      const items = ctx.state.profiles || [];
      ui.$('[data-role="summary"]', root).textContent = items.length + ' 个配置 · ' +
        items.filter(function (item) { return item.active; }).length + ' 个使用中';
      if (!items.length) {
        list.innerHTML = ui.empty('还没有配置，导入订阅开始使用', 'profiles');
        return;
      }
      list.innerHTML = items.map(function (item) {
        const share = Number(item.usedShare) || 0;
        return '<div class="profile-item' + (item.active ? ' is-active' : '') + '">' +
          '<div class="profile-main">' +
            '<div class="row"><span class="profile-name">' + ui.esc(item.name) + '</span>' +
              (item.active ? '<span class="chip is-ok"><i class="chip-dot"></i>使用中</span>' : '') +
              '<span class="chip">' + ui.esc(item.type === 'file' ? '本地' : '订阅') + '</span></div>' +
            '<div class="profile-meta"><span>更新于 ' + ui.esc(item.updatedAt || '—') + '</span>' +
              (item.expireDays ? '<span>剩余 ' + ui.esc(item.expireDays) + ' 天</span>' : '') +
              '<span>' + ui.esc(item.proxies || 0) + ' 节点 · ' + ui.esc(item.rules || 0) + ' 规则</span></div>' +
            '<div class="profile-usage"><div class="usage-row"><strong>' + Math.round(share * 100) + '% 已用</strong>' +
              '<span>' + ui.esc(item.used || '0 B') + ' / ' + ui.esc(item.total || '0 B') + '</span></div>' +
              '<svg class="wave" viewBox="0 0 300 12" preserveAspectRatio="none">' + ui.wave(share, 300, 12) + '</svg></div>' +
          '</div>' +
          '<div class="profile-actions">' +
            (item.active ? '' : '<button class="btn" data-apply="' + ui.esc(item.id) + '" type="button">启用</button>') +
            '<button class="btn is-danger" data-delete="' + ui.esc(item.id) + '" type="button">删除</button>' +
          '</div>' +
        '</div>';
      }).join('');

      ui.$$('[data-apply]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          button.disabled = true;
          api.applyProfile(button.getAttribute('data-apply'))
            .then(function () { ui.toast('配置已启用', 'ok'); return ctx.reloadProfiles(); })
            .catch(function (error) { ui.toast('启用失败: ' + error.message, 'error'); button.disabled = false; });
        });
      });
      ui.$$('[data-delete]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const item = (ctx.state.profiles || []).find(function (p) { return p.id === button.getAttribute('data-delete'); });
          ui.openSheet('删除配置', '<p class="sheet-note">确定删除「' + ui.esc(item ? item.name : '') + '」吗？该操作不可撤销。</p>' +
            '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
            '<button class="btn is-danger" data-confirm type="button">删除</button></div>', function (body) {
            ui.$('[data-cancel]', body).addEventListener('click', ui.closeSheet);
            ui.$('[data-confirm]', body).addEventListener('click', function () {
              api.deleteProfile(button.getAttribute('data-delete'))
                .then(function () { ui.closeSheet(); ui.toast('配置已删除'); return ctx.reloadProfiles(); })
                .catch(function (error) { ui.toast('删除失败: ' + error.message, 'error'); });
            });
          });
        });
      });
    }

    function openImport() {
      ui.openSheet('导入配置',
        '<div class="field"><label for="import-url">订阅链接</label>' +
          '<input id="import-url" class="input" type="url" placeholder="https://example.com/sub" autocomplete="off"></div>' +
          '<div class="field"><label for="import-path">或配置文件路径</label>' +
          '<input id="import-path" class="input" type="text" placeholder="/etc/flclash/config.yaml" autocomplete="off"></div>' +
          '<p class="sheet-note">支持 Clash / mihomo YAML。导入后会生成新配置，需在列表中选择启用。</p>' +
          '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
          '<button class="btn is-primary" data-confirm type="button">导入</button></div>',
        function (body) {
          const urlInput = ui.$('#import-url', body);
          const pathInput = ui.$('#import-path', body);
          const submit = function () {
            const value = (urlInput.value || pathInput.value || '').trim();
            if (!value) { ui.toast('请输入链接或路径', 'error'); urlInput.focus(); return; }
            ui.$('[data-confirm]', body).disabled = true;
            api.importProfile(value)
              .then(function () { ui.closeSheet(); ui.toast('配置已导入', 'ok'); return ctx.reloadProfiles(); })
              .catch(function (error) {
                ui.toast('导入失败: ' + error.message, 'error');
                ui.$('[data-confirm]', body).disabled = false;
              });
          };
          ui.$('[data-cancel]', body).addEventListener('click', ui.closeSheet);
          ui.$('[data-confirm]', body).addEventListener('click', submit);
          urlInput.addEventListener('keydown', function (event) { if (event.key === 'Enter') submit(); });
          pathInput.addEventListener('keydown', function (event) { if (event.key === 'Enter') submit(); });
        });
    }

    ui.$('[data-act="import"]', root).addEventListener('click', openImport);
    render();
    ctx.onProfiles = render;
    return root;
  }

  /* ------------------------------------------------------------- requests */

  function requests(ctx) {
    const api = ctx.api;
    const root = el('<div class="view page"></div>');
    let paused = false;
    let keyword = '';

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="search">' + icon('search') + '<input class="input" type="search" placeholder="筛选主机或规则" aria-label="筛选连接"></div>' +
        '<button class="btn" data-act="pause" type="button">' + icon('pause') + '暂停</button>' +
        '<button class="btn is-ghost" data-act="clear" type="button">' + icon('trash') + '清空</button>' +
      '</div>' +
      '<section class="card is-flush">' +
        '<div class="card-head"><span class="glyph">' + icon('requests') + '</span><span class="card-title">实时连接</span>' +
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
        '<th>主机</th><th>规则</th><th>链路</th><th class="num">上传</th><th class="num">下载</th><th>时长</th><th></th>' +
        '</tr></thead><tbody>' + data.slice(0, 80).map(function (item) {
          return '<tr>' +
            '<td title="' + ui.esc(item.host) + '">' + ui.esc(item.host) + ':' + ui.esc(item.port) + '<div class="muted" style="font-size:11px">' + ui.esc(item.source || '') + '</div></td>' +
            '<td class="is-muted">' + ui.esc(item.rule || '-') + '</td>' +
            '<td>' + ui.esc(item.chain || '-') + '</td>' +
            '<td class="num">' + ui.bytes(item.up) + '</td>' +
            '<td class="num">' + ui.bytes(item.down) + '</td>' +
            '<td class="is-muted">' + ui.duration(item.duration) + '</td>' +
            '<td><button class="icon-button" data-close="' + ui.esc(item.id) + '" type="button" aria-label="关闭连接">' + icon('close') + '</button></td>' +
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
      pauseBtn.innerHTML = icon(paused ? 'play' : 'pause') + (paused ? '继续' : '暂停');
      if (!paused) render();
    });

    ui.$('[data-act="clear"]', root).addEventListener('click', function () {
      ctx.state.connections = [];
      render();
      ui.toast('已清空当前视图');
    });

    render();
    ctx.onConnections = render;
    return root;
  }

  /* ----------------------------------------------------------------- logs */

  function logs(ctx) {
    const root = el('<div class="view page"></div>');
    let level = '';
    let paused = false;

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="seg-chips" data-role="levels"></div>' +
        '<div class="grow"></div>' +
        '<button class="btn" data-act="pause" type="button">' + icon('pause') + '暂停</button>' +
        '<button class="btn is-ghost" data-act="export" type="button">' + icon('down') + '导出</button>' +
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
      this.innerHTML = icon(paused ? 'play' : 'pause') + (paused ? '继续' : '暂停');
      if (!paused) render(true);
    });

    ui.$('[data-act="export"]', root).addEventListener('click', function () {
      const text = (ctx.state.logs || []).map(function (item) {
        return new Date(item.time).toISOString() + ' [' + item.level + '] ' + item.message;
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
    return root;
  }

  /* ------------------------------------------------------------------ dns */

  function dns(ctx) {
    const root = el('<div class="view page"></div>');
    root.innerHTML =
      '<div class="page-toolbar"><div class="grow muted" data-role="summary">—</div>' +
        '<button class="btn" data-act="flush" type="button">' + icon('trash') + '清空缓存</button></div>' +
      '<section class="card is-flush"><div class="card-head"><span class="glyph">' + icon('dns') + '</span>' +
        '<span class="card-title">DNS 查询</span><span class="card-end"><span class="chip" data-role="count">0</span></span></div>' +
        '<div class="table-wrap" data-role="table"></div></section>';

    function render() {
      const data = ctx.state.dns || [];
      ui.$('[data-role="count"]', root).textContent = data.length;
      ui.$('[data-role="summary"]', root).textContent = '最近 ' + data.length + ' 条查询记录';
      const wrap = ui.$('[data-role="table"]', root);
      if (!data.length) { wrap.innerHTML = ui.empty('暂无 DNS 查询', 'dns'); return; }
      wrap.innerHTML = '<table class="data"><thead><tr><th>时间</th><th>域名</th><th>类型</th><th>结果</th><th>解析器</th><th class="num">耗时</th></tr></thead><tbody>' +
        data.slice(0, 100).map(function (item) {
          const fake = String(item.answer || '').indexOf('198.18.') === 0;
          return '<tr><td class="is-muted">' + ui.esc(ui.clock(item.time)) + '</td>' +
            '<td>' + ui.esc(item.name) + '</td>' +
            '<td class="is-muted">' + ui.esc(item.type || 'A') + '</td>' +
            '<td class="dns-answer' + (fake ? ' is-fake' : '') + '">' + ui.esc(item.answer || '-') + '</td>' +
            '<td class="is-muted">' + ui.esc(item.resolver || '-') + '</td>' +
            '<td class="num">' + (item.elapsed ? ui.esc(item.elapsed) + ' ms' : '-') + '</td></tr>';
        }).join('') + '</tbody></table>';
    }

    ui.$('[data-act="flush"]', root).addEventListener('click', function () {
      ctx.api.flushDns()
        .then(function () { ui.toast('DNS 缓存已清空', 'ok'); ctx.state.dns = []; render(); })
        .catch(function (error) { ui.toast('清空失败: ' + error.message, 'error'); });
    });

    render();
    ctx.onDns = render;
    return root;
  }

  /* ---------------------------------------------------------------- tools */

  function tools(ctx) {
    const root = el('<div class="view page"></div>');
    const items = [
      ['dns', 'DNS 查询', '查看解析记录与命中策略', 'dns'],
      ['logs', '运行日志', '内核事件与排障信息', 'logs'],
      ['requests', '实时连接', '当前活动连接与流量', 'requests'],
      ['settings', '设置', '网络、内核与外观参数', 'settings']
    ];
    root.innerHTML = '<div class="tool-grid">' + items.map(function (item) {
      return '<button class="tool-card" data-go="' + item[0] + '" type="button">' +
        '<span class="tile-icon">' + icon(item[3]) + '</span>' +
        '<span class="col" style="align-items:flex-start;gap:2px"><span class="tool-title">' + item[1] + '</span>' +
        '<span class="tool-desc">' + item[2] + '</span></span>' +
        '<span class="chevron" style="margin-left:auto">' + icon('chevron') + '</span></button>';
    }).join('') + '</div>' +
      '<section class="card"><div class="card-head"><span class="glyph">' + icon('tools') + '</span><span class="card-title">维护</span></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">重启内核</div><div class="row-desc">重新加载配置并重建所有连接</div></div>' +
          '<button class="btn" data-act="restart" type="button">重启</button></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">刷新订阅</div><div class="row-desc">重新下载当前启用的配置</div></div>' +
          '<button class="btn" data-act="refresh" type="button">刷新</button></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">清空 DNS 缓存</div><div class="row-desc">清除 fake-ip 与解析缓存</div></div>' +
          '<button class="btn" data-act="flush" type="button">清空</button></div>' +
      '</section>' +
      '<section class="card"><div class="card-head"><span class="glyph">' + icon('info') + '</span><span class="card-title">关于</span></div>' +
        '<div class="kv"><span class="k">应用版本</span><span class="v" data-role="version">—</span></div>' +
        '<div class="kv"><span class="k">内核版本</span><span class="v" data-role="core">—</span></div>' +
        '<div class="kv"><span class="k">WebUI</span><span class="v">FlClash WebUI · LuCI launcher</span></div>' +
      '</section>';

    ui.$$('[data-go]', root).forEach(function (button) {
      button.addEventListener('click', function () { ctx.navigate(button.getAttribute('data-go')); });
    });

    const status = ctx.state.status || {};
    ui.$('[data-role="version"]', root).textContent = status.version || '—';
    ui.$('[data-role="core"]', root).textContent = status.coreVersion || '—';

    ui.$('[data-act="restart"]', root).addEventListener('click', function () {
      ctx.api.service('restart')
        .then(function () { ui.toast('内核已重启', 'ok'); return ctx.refreshStatus(); })
        .catch(function (error) { ui.toast('重启失败: ' + error.message, 'error'); });
    });
    ui.$('[data-act="refresh"]', root).addEventListener('click', function () {
      const active = (ctx.state.profiles || []).find(function (item) { return item.active; });
      if (!active) { ui.toast('没有启用的配置', 'error'); return; }
      ctx.api.applyProfile(active.id)
        .then(function () { ui.toast('订阅已刷新', 'ok'); return ctx.reloadProfiles(); })
        .catch(function (error) { ui.toast('刷新失败: ' + error.message, 'error'); });
    });
    ui.$('[data-act="flush"]', root).addEventListener('click', function () {
      ctx.api.flushDns()
        .then(function () { ui.toast('DNS 缓存已清空', 'ok'); })
        .catch(function (error) { ui.toast('清空失败: ' + error.message, 'error'); });
    });
    return root;
  }

  /* ------------------------------------------------------------- settings */

  function settings(ctx) {
    const root = el('<div class="view page"></div>');
    const cfg = ctx.state.config || {};
    const status = ctx.state.status || {};

    root.innerHTML =
      '<div class="settings-group"><div class="settings-title">网络</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">出站模式</div><div class="row-desc">规则 / 全局 / 直连</div></div>' +
            '<select class="select" data-cfg="mode"><option value="rule">规则</option><option value="global">全局</option><option value="direct">直连</option></select></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">混合端口</div><div class="row-desc">HTTP / SOCKS 共用端口</div></div>' +
            '<input class="input" type="number" min="1" max="65535" data-cfg="mixedPort"></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">允许局域网连接</div><div class="row-desc">其它设备可经本机代理</div></div>' +
            '<button class="switch" role="switch" data-cfg="allowLan" type="button"></button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">IPv6</div><div class="row-desc">解析并代理 IPv6 流量</div></div>' +
            '<button class="switch" role="switch" data-cfg="ipv6" type="button"></button></div>' +
        '</div></div>' +
      '<div class="settings-group"><div class="settings-title">内核</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">虚拟网卡（TUN）</div><div class="row-desc">接管全局流量，需内核支持</div></div>' +
            '<button class="switch" role="switch" data-cfg="tun" type="button"></button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">系统代理</div><div class="row-desc">写入系统代理设置</div></div>' +
            '<button class="switch" role="switch" data-cfg="systemProxy" type="button"></button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">日志级别</div><div class="row-desc">影响日志详细程度</div></div>' +
            '<select class="select" data-cfg="logLevel"><option value="debug">debug</option><option value="info">info</option><option value="warning">warning</option><option value="error">error</option></select></div>' +
        '</div></div>' +
      '<div class="settings-group"><div class="settings-title">外观</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">深色模式</div><div class="row-desc">跟随或手动切换</div></div>' +
            '<button class="switch" role="switch" data-cfg="theme" type="button"></button></div>' +
        '</div></div>' +
      '<div class="settings-group"><div class="settings-title">服务</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">运行状态</div><div class="row-desc" data-role="state">—</div></div>' +
            '<button class="btn" data-act="toggle" type="button">—</button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">重启内核</div><div class="row-desc">保留配置，重建连接</div></div>' +
            '<button class="btn" data-act="restart" type="button">重启</button></div>' +
        '</div></div>' +
      '<div class="page-toolbar" style="justify-content:flex-end"><button class="btn is-primary" data-act="save" type="button">保存设置</button></div>';

    const controls = {
      mode: ui.$('[data-cfg="mode"]', root),
      mixedPort: ui.$('[data-cfg="mixedPort"]', root),
      logLevel: ui.$('[data-cfg="logLevel"]', root),
      allowLan: ui.$('[data-cfg="allowLan"]', root),
      ipv6: ui.$('[data-cfg="ipv6"]', root),
      tun: ui.$('[data-cfg="tun"]', root),
      systemProxy: ui.$('[data-cfg="systemProxy"]', root),
      theme: ui.$('[data-cfg="theme"]', root)
    };

    controls.mode.value = cfg.mode || status.mode || 'rule';
    controls.mixedPort.value = cfg.mixedPort || status.mixedPort || 7890;
    controls.logLevel.value = cfg.logLevel || status.logLevel || 'info';
    ['allowLan', 'ipv6', 'tun', 'systemProxy'].forEach(function (key) {
      controls[key].classList.toggle('is-on', !!(cfg[key] != null ? cfg[key] : status[key]));
      controls[key].setAttribute('aria-checked', String(!!(cfg[key] != null ? cfg[key] : status[key])));
      controls[key].addEventListener('click', function () { controls[key].classList.toggle('is-on'); });
    });
    controls.theme.classList.toggle('is-on', document.documentElement.getAttribute('data-theme') === 'dark');
    controls.theme.addEventListener('click', function () {
      const dark = !controls.theme.classList.contains('is-on');
      controls.theme.classList.toggle('is-on', dark);
      ctx.setTheme(dark ? 'dark' : 'light');
    });

    function paintState() {
      const running = !!(ctx.state.status && ctx.state.status.running);
      ui.$('[data-role="state"]', root).textContent = running ? '运行中' : '已停止';
      const toggle = ui.$('[data-act="toggle"]', root);
      toggle.textContent = running ? '停止' : '启动';
      toggle.classList.toggle('is-danger', running);
    }

    paintState();

    ui.$('[data-act="toggle"]', root).addEventListener('click', function () {
      const running = !!(ctx.state.status && ctx.state.status.running);
      ctx.api.service(running ? 'stop' : 'start')
        .then(function () { ui.toast(running ? '内核已停止' : '内核已启动', 'ok'); return ctx.refreshStatus(); })
        .then(paintState)
        .catch(function (error) { ui.toast('操作失败: ' + error.message, 'error'); });
    });

    ui.$('[data-act="restart"]', root).addEventListener('click', function () {
      ctx.api.service('restart')
        .then(function () { ui.toast('内核已重启', 'ok'); return ctx.refreshStatus(); })
        .catch(function (error) { ui.toast('重启失败: ' + error.message, 'error'); });
    });

    ui.$('[data-act="save"]', root).addEventListener('click', function (event) {
      const button = event.currentTarget;
      button.disabled = true;
      const patch = {
        mode: controls.mode.value,
        mixedPort: Number(controls.mixedPort.value) || 7890,
        logLevel: controls.logLevel.value,
        allowLan: controls.allowLan.classList.contains('is-on'),
        ipv6: controls.ipv6.classList.contains('is-on'),
        tun: controls.tun.classList.contains('is-on'),
        systemProxy: controls.systemProxy.classList.contains('is-on')
      };
      ctx.api.saveConfig(patch)
        .then(function () {
          ui.toast('设置已保存', 'ok');
          return ctx.refreshStatus();
        })
        .catch(function (error) { ui.toast('保存失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; });
    });

    return root;
  }

  return {
    proxies: proxies,
    profiles: profiles,
    requests: requests,
    logs: logs,
    dns: dns,
    tools: tools,
    settings: settings
  };
})();
