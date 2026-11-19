/* View: 配置 — profiles with real sync/update, edit (url/interval), apply and
 * delete. The update button re-downloads the subscription and re-applies it
 * when active (desktop-client behaviour). */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function profiles(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';
    root.innerHTML =
      '<div class="page-toolbar"><div class="grow muted" data-role="summary">—</div>' +
        '<button class="btn is-primary" data-act="import" type="button">' + icon('plus', 16) + '导入订阅</button></div>' +
      '<div class="profile-list" data-role="list"></div>';

    const list = ui.$('[data-role="list"]', root);

    function render() {
      const items = ctx.state.profiles || [];
      ui.$('[data-role="summary"]', root).textContent =
        items.length + ' 个配置 · ' + items.filter(function (item) { return item.active; }).length + ' 个使用中';
      if (!items.length) {
        list.innerHTML = ui.empty('还没有配置,导入订阅开始使用', 'profiles');
        return;
      }
      list.innerHTML = items.map(function (item) {
        const share = Number(item.usedShare) || 0;
        return '<div class="profile-item' + (item.active ? ' is-active' : '') + '">' +
          '<div class="profile-main">' +
            '<div class="row"><span class="profile-name">' + ui.esc(item.name) + '</span>' +
              (item.active ? '<span class="chip is-ok"><i class="chip-dot"></i>使用中</span>' : '') +
              '<span class="chip">' + (item.type === 'file' ? '本地' : '订阅') + '</span>' +
              (item.interval >= 60 ? '<span class="chip chip-plain">自动 ' + Math.round(item.interval / 60) + ' 分钟</span>' : '') + '</div>' +
            '<div class="profile-meta"><span>更新于 ' + ui.esc(item.updatedAt || '—') + '</span>' +
              (item.expireDays ? '<span>剩余 ' + ui.esc(item.expireDays) + ' 天</span>' : '') +
              '<span>' + ui.esc(item.proxies || 0) + ' 节点 · ' + ui.esc(item.groups || 0) + ' 组 · ' + ui.esc(item.rules || 0) + ' 规则</span></div>' +
            '<div class="profile-usage"><div class="usage-row"><strong>' + (share * 100).toFixed(1).replace(/\.0$/, '') + '% 已用</strong>' +
              '<span>' + ui.esc(item.used || '0 B') + ' / ' + ui.esc(item.total || '0 B') + '</span></div>' +
              '<svg class="wave" viewBox="0 0 300 12" preserveAspectRatio="none">' + ui.wave(share, 300, 12) + '</svg></div>' +
          '</div>' +
          '<div class="profile-actions">' +
            '<button class="icon-button" data-update="' + ui.esc(item.id) + '" type="button" title="更新订阅">' + icon('sync', 16) + '</button>' +
            '<button class="icon-button" data-edit="' + ui.esc(item.id) + '" type="button" title="编辑">' + icon('edit', 16) + '</button>' +
            (item.active ? '' : '<button class="btn" data-apply="' + ui.esc(item.id) + '" type="button">启用</button>') +
            '<button class="icon-button is-danger" data-delete="' + ui.esc(item.id) + '" type="button" title="删除">' + icon('trash', 16) + '</button>' +
          '</div>' +
        '</div>';
      }).join('');

      ui.$$('[data-update]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const id = button.getAttribute('data-update');
          button.classList.add('is-spinning');
          api.updateProfile(id)
            .then(function () { ui.toast('订阅已更新', 'ok'); return ctx.reloadProfiles(); })
            .catch(function (error) { ui.toast('更新失败: ' + error.message, 'error'); })
            .then(function () { button.classList.remove('is-spinning'); });
        });
      });

      ui.$$('[data-edit]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const id = button.getAttribute('data-edit');
          const item = (ctx.state.profiles || []).find(function (p) { return p.id === id; }) || {};
          ui.openSheet('编辑配置',
            '<div class="field"><label>名称</label><input id="edit-name" class="input" type="text" value="' + ui.esc(item.name || '') + '"></div>' +
            '<div class="field"><label>订阅链接</label><input id="edit-url" class="input" type="url" value="' + ui.esc(item.url || '') + '" placeholder="https://…"></div>' +
            '<div class="field"><label>自动更新(分钟,0 关闭)</label><input id="edit-interval" class="input" type="number" min="0" step="5" value="' + Math.round((item.interval || 0) / 60) + '"></div>' +
            '<p class="sheet-note">自动更新按订阅间隔重新下载并转换;启用中的配置会随后自动重载内核。</p>' +
            '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
            '<button class="btn is-primary" data-confirm type="button">保存</button></div>',
            function (body, close, helpers) {
              ui.$('[data-cancel]', body).addEventListener('click', close);
              ui.$('[data-confirm]', body).addEventListener('click', function () {
                const name = ui.$('#edit-name', body).value.trim();
                const url = ui.$('#edit-url', body).value.trim();
                const minutes = Number(ui.$('#edit-interval', body).value) || 0;
                api.editProfile(id, { name: name, url: url, interval: minutes * 60 })
                  .then(function () { helpers.markSaved(); close(); ui.toast('配置已保存', 'ok'); return ctx.reloadProfiles(); })
                  .catch(function (error) { ui.toast('保存失败: ' + error.message, 'error'); });
              });
            });
        });
      });

      ui.$$('[data-apply]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          button.disabled = true;
          api.applyProfile(button.getAttribute('data-apply'))
            .then(function () { ui.toast('配置已启用,内核已加载', 'ok'); return ctx.reloadProfiles(); })
            .catch(function (error) { ui.toast('启用失败: ' + error.message, 'error'); button.disabled = false; });
        });
      });

      ui.$$('[data-delete]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const id = button.getAttribute('data-delete');
          const item = (ctx.state.profiles || []).find(function (p) { return p.id === id; }) || {};
          ui.confirmSheet('删除配置', '确定删除「' + (item.name || id) + '」吗?该操作不可撤销。', function () {
            api.deleteProfile(id)
              .then(function () { ui.toast('配置已删除'); return ctx.reloadProfiles(); })
              .catch(function (error) { ui.toast('删除失败: ' + error.message, 'error'); });
          });
        });
      });
    }

    function openImport() {
      ui.openSheet('导入订阅',
        '<div class="field"><label for="import-url">订阅链接 / 分享链接</label>' +
          '<input id="import-url" class="input" type="url" placeholder="https://… 或 ss:// vmess:// vless:// trojan://" autocomplete="off"></div>' +
        '<p class="sheet-note">支持 Clash/mihomo YAML、base64 节点列表以及 ss:// ssr:// vmess:// vless:// trojan:// hysteria2:// tuic:// 链接,导入时自动转换为内核配置。</p>' +
        '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
        '<button class="btn is-primary" data-confirm type="button">导入</button></div>',
        function (body, close, helpers) {
          const urlInput = ui.$('#import-url', body);
          const submit = function () {
            const value = (urlInput.value || '').trim();
            if (!value) { ui.toast('请输入链接', 'error'); urlInput.focus(); return; }
            ui.$('[data-confirm]', body).disabled = true;
            api.importProfile(value)
              .then(function (item) {
                helpers.markSaved();
                close();
                ui.toast('已导入 ' + (item.proxies || 0) + ' 个节点', 'ok');
                return ctx.reloadProfiles();
              })
              .catch(function (error) {
                ui.toast('导入失败: ' + error.message, 'error');
                ui.$('[data-confirm]', body).disabled = false;
              });
          };
          ui.$('[data-cancel]', body).addEventListener('click', close);
          ui.$('[data-confirm]', body).addEventListener('click', submit);
          urlInput.addEventListener('keydown', function (event) { if (event.key === 'Enter') submit(); });
        });
    }

    ui.$('[data-act="import"]', root).addEventListener('click', openImport);
    api.profiles().then(function (data) { ctx.state.profiles = data; render(); }).catch(function (error) {
      list.innerHTML = ui.empty(error.message, 'info');
    });
    ctx.onProfiles = render;
    ui.every('profiles', function () {
      api.profiles().then(function (data) { ctx.state.profiles = data; render(); }).catch(function () {});
    }, 8000);
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.profiles = profiles;
})();
