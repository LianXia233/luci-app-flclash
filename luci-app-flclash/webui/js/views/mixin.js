/* View: 配置混入 (Mixin) — Nikki-style mixin.yaml with deep merge, live
 * validation and a merged preview. Saving only persists; applying happens
 * on the next profile apply/update (stated explicitly in the UI). */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  const SAMPLE = '# 混入示例：覆盖/追加到订阅生成的配置\n# mode: rule\n# dns:\n#   ipv6: true\n# rules:\n#   - DOMAIN-SUFFIX,example.com,节点选择\n';

  function mixin(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';
    let dirty = false;
    let lastText = '';

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="grow muted" data-role="state">—</div>' +
        '<button class="btn" data-act="sample" type="button">' + icon('file', 16) + '示例</button>' +
        '<button class="btn" data-act="check" type="button">' + icon('check', 16) + '校验预览</button>' +
        '<button class="btn is-primary" data-act="save" type="button">' + icon('check', 16) + '保存</button>' +
      '</div>' +
      '<section class="card">' +
        '<div class="card-head"><span class="glyph" data-icon="tune"></span><span class="card-title">mixin.yaml</span>' +
        '<span class="card-end"><span class="chip" data-role="badge">未启用</span></span></div>' +
        '<textarea class="input dns-area editor-area" data-role="text" spellcheck="false" ' +
          'placeholder="留空表示不启用混入"></textarea>' +
        '<p class="sheet-note">映射按键递归合并；<code>rules</code> / <code>rule-providers</code> 的条目会插到最前（用户规则优先）；' +
        '其它数组整体替换；值为 <code>null</code> 表示删除该键。保存后在下次「启用 / 更新订阅」时生效。</p>' +
      '</section>' +
      '<section class="card is-flush" hidden data-role="report">' +
        '<div class="card-head"><span class="glyph" data-icon="check"></span><span class="card-title">校验结果</span></div>' +
        '<div data-role="report-body"></div>' +
      '</section>';

    const area = ui.$('[data-role="text"]', root);
    const badge = ui.$('[data-role="badge"]', root);
    const stateLine = ui.$('[data-role="state"]', root);
    const report = ui.$('[data-role="report"]', root);
    const reportBody = ui.$('[data-role="report-body"]', root);

    function paintState() {
      const enabled = area.value.trim().length > 0;
      badge.textContent = enabled ? (dirty ? '已修改 · 未保存' : '已启用') : '未启用';
      badge.className = 'chip ' + (enabled ? (dirty ? 'is-warn' : 'is-ok') : '');
      stateLine.textContent = enabled ? '混入将在下次应用配置时合并' : '当前没有混入内容';
    }

    area.addEventListener('input', function () { dirty = area.value !== lastText; paintState(); });

    api.mixin().then(function (data) {
      area.value = data.text || '';
      lastText = area.value;
      dirty = false;
      paintState();
    }).catch(function (error) {
      stateLine.textContent = error.message;
    });

    function renderReport(data) {
      report.hidden = false;
      const merged = data.merged || {};
      const base = data.base || {};
      const keys = data.mixinKeys || [];
      const rows = [];
      rows.push('<div class="settings-row"><div class="row-copy"><div class="row-title">合并后配置</div>' +
        '<div class="row-desc">' + (merged.ok ? '校验通过' : '存在错误') +
        (keys.length ? ' · 混入键: ' + ui.esc(keys.join(', ')) : ' · 未启用混入') + '</div></div>' +
        '<span class="chip ' + (merged.ok ? 'is-ok' : 'is-bad') + '">' + (merged.ok ? '可应用' : '不可应用') + '</span></div>');
      (merged.errors || []).forEach(function (message) {
        rows.push('<div class="settings-row"><div class="row-copy"><div class="row-desc" style="color:var(--bad)">' + ui.esc(message) + '</div></div></div>');
      });
      (merged.warnings || []).concat(base.warnings || []).forEach(function (message) {
        rows.push('<div class="settings-row"><div class="row-copy"><div class="row-desc">' + ui.esc(message) + '</div></div></div>');
      });
      rows.push('<div class="settings-row"><div class="row-copy"><div class="row-title">预览</div>' +
        '<div class="row-desc">合并后的完整 YAML（只读）</div></div>' +
        '<button class="btn" data-act="copy-preview" type="button">' + icon('copy', 15) + '复制</button></div>' +
        '<textarea class="input dns-area editor-area" readonly data-role="preview">' + ui.esc(data.preview || '') + '</textarea>');
      reportBody.innerHTML = rows.join('');
      const copyBtn = ui.$('[data-act="copy-preview"]', reportBody);
      if (copyBtn) copyBtn.addEventListener('click', function () { ctx.copy(data.preview || ''); });
    }

    ui.$('[data-act="check"]', root).addEventListener('click', function () {
      const button = this;
      button.disabled = true;
      api.validateConfig({ text: area.value })
        .then(renderReport)
        .catch(function (error) { ui.toast('校验失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; });
    });

    ui.$('[data-act="sample"]', root).addEventListener('click', function () {
      if (area.value.trim() && area.value !== SAMPLE) {
        ui.confirmSheet('插入示例', '当前已有内容，追加示例注释到末尾？', function () {
          area.value = area.value.replace(/\s*$/, '') + '\n\n' + SAMPLE;
          dirty = true;
          paintState();
        });
        return;
      }
      area.value = SAMPLE;
      dirty = true;
      paintState();
    });

    ui.$('[data-act="save"]', root).addEventListener('click', function () {
      const button = this;
      button.disabled = true;
      api.saveMixin(area.value)
        .then(function () {
          lastText = area.value;
          dirty = false;
          paintState();
          ui.toast('混入已保存，下次应用配置时生效', 'ok');
        })
        .catch(function (error) { ui.toast('保存失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; });
    });

    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.mixin = mixin;
})();
