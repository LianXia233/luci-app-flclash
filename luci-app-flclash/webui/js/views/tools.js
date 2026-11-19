/* View: 工具 — aggregate entries + maintenance actions (all real). */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function tools(ctx) {
    const root = document.createElement('div');
    root.className = 'view page';
    const items = [
      ['dns', 'DNS 测试', '通过内核实时解析并查看结果', 'dns'],
      ['logs', '运行日志', '内核事件与排障信息', 'logs'],
      ['rules', '规则', '当前生效的分流规则', 'split'],
      ['requests', '实时连接', '当前活动连接与流量', 'requests'],
      ['settings', '设置', '网络、内核与外观参数', 'settings'],
      ['backup', '备份与恢复', '快照、下载与一键恢复配置', 'backup'],
      ['mixin', '配置混入', '规则/DNS 覆写与合并预览', 'tune'],
      ['acl', '访问控制', '局域网黑白名单与端口/网段排除', 'acl'],
      ['dnsAdvanced', 'DNS 进阶', 'Fallback、分组解析与分域名策略', 'dns']
    ];
    root.innerHTML = '<div class="tool-grid">' + items.map(function (item) {
      return '<button class="tool-card" data-go="' + item[0] + '" type="button">' +
        '<span class="tile-icon" data-icon="' + item[3] + '"></span>' +
        '<span class="col" style="align-items:flex-start;gap:2px"><span class="tool-title">' + item[1] + '</span>' +
        '<span class="tool-desc">' + item[2] + '</span></span>' +
        '<span class="chevron" data-icon="chevron"></span></button>';
    }).join('') + '</div>' +
      '<section class="card"><div class="card-head"><span class="glyph" data-icon="tools"></span><span class="card-title">维护</span></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">重启内核</div><div class="row-desc">重新加载配置并重建所有连接</div></div>' +
          '<button class="btn" data-act="restart" type="button">重启</button></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">更新当前订阅</div><div class="row-desc">重新下载、转换并重载启用中的配置</div></div>' +
          '<button class="btn" data-act="refresh" type="button">更新</button></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">清空 DNS 缓存</div><div class="row-desc">清除 fake-ip 与解析缓存</div></div>' +
          '<button class="btn" data-act="flush" type="button">清空</button></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">更新 Geo 数据</div><div class="row-desc">由内核下载最新 GeoIP / GeoSite 数据库</div></div>' +
          '<button class="btn" data-act="geo" type="button">更新</button></div>' +
      '</section>' +
      '<section class="card"><div class="card-head"><span class="glyph" data-icon="info"></span><span class="card-title">关于</span></div>' +
        '<div class="kv"><span class="k">应用版本</span><span class="v" data-role="version">—</span></div>' +
        '<div class="kv"><span class="k">内核版本</span><span class="v" data-role="core">—</span></div>' +
        '<div class="kv"><span class="k">后端</span><span class="v">flclash-api (Rust/Tokio)</span></div>' +
        '<div class="kv"><span class="k">WebUI</span><span class="v">FlClash WebUI · LuCI</span></div>' +
      '</section>';

    ui.$$('[data-icon]', root);
    window.FlClashIcons.mount(root);

    ui.$$('[data-go]', root).forEach(function (button) {
      button.addEventListener('click', function () { ctx.navigate(button.getAttribute('data-go')); });
    });

    const status = ctx.state.status || {};
    ui.$('[data-role="version"]', root).textContent = status.apiVersion || '—';
    ui.$('[data-role="core"]', root).textContent = status.coreVersion || '—';
    // Live controller version wins over the sampled status; when the core is
    // stopped say so instead of leaving a stale or empty value.
    ctx.api.version().then(function (info) {
      const node = ui.$('[data-role="core"]', root);
      if (!node) return;
      if (info && info.running === false) node.textContent = '未运行';
      else if (info && info.version) node.textContent = info.version + (info.meta ? ' (Meta)' : '');
    }).catch(function () { /* keep sampled value */ });

    ui.$('[data-act="geo"]', root).addEventListener('click', function () {
      const button = this;
      button.disabled = true;
      button.textContent = '更新中…';
      const done = function (ok, message) {
        button.disabled = false;
        button.textContent = '更新';
        ui.toast(message, ok ? 'ok' : 'error');
      };
      ctx.api.geoUpdate().then(function (reply) {
        if (reply && reply.taskId && ctx.comm) {
          return ctx.comm.waitTask(reply.taskId, 120000).then(function (task) {
            if (task && task.state === 'failed') throw new Error(task.error || '更新失败');
            done(true, 'Geo 数据已更新');
          });
        }
        done(true, 'Geo 数据更新已提交');
      }).catch(function (error) { done(false, '更新失败: ' + error.message); });
    });

    ui.$('[data-act="restart"]', root).addEventListener('click', function () {
      const button = this;
      button.disabled = true;
      ctx.api.service('restart')
        .then(function () { ui.toast('内核已重启', 'ok'); return ctx.refreshStatus(); })
        .catch(function (error) { ui.toast('重启失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; });
    });

    ui.$('[data-act="refresh"]', root).addEventListener('click', function () {
      const button = this;
      const active = (ctx.state.profiles || []).find(function (item) { return item.active; });
      if (!active) { ui.toast('没有启用的配置', 'error'); return; }
      button.disabled = true;
      ctx.api.updateProfile(active.id)
        .then(function () { ui.toast('订阅已更新并重载', 'ok'); return ctx.refreshStatus(); })
        .catch(function (error) { ui.toast('更新失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; });
    });

    ui.$('[data-act="flush"]', root).addEventListener('click', function () {
      ctx.api.flushDns()
        .then(function () { ui.toast('DNS 缓存已清空', 'ok'); })
        .catch(function (error) { ui.toast('清空失败: ' + error.message, 'error'); });
    });

    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.tools = tools;
})();
