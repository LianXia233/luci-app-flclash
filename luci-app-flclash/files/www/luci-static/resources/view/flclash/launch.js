/* LuCI FlClash — single entry page under admin/services/flclash.
 *
 * Tabs: 服务(状态开关/更新内核/一键进 WebUI) + 日志(logread tail)。
 * All plugin functions live inside this one menu node — no sibling pages.
 */
'use strict';
'require view';
'require ui';
'require rpc';
'require poll';
'require dom';

/* This firmware's dispatcher answers admin/ubus single calls as pretty
 * text/plain JSON, which the bundled rpc.js never parses (calls hang).
 * A tiny direct fetch around the same trusted endpoint is the reliable
 * path — verified against the live device. */
function ubusCall(object, method, params) {
	return fetch(L.url('admin/ubus'), {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({
			jsonrpc: '2.0', id: Date.now(), method: 'call',
			params: [L.env.sessionid, object, method, params || {}]
		})
	}).then(function (r) { return r.json(); }).then(function (data) {
		const res = (data && data.result) || [];
		if (res[0] !== 0)
			throw new Error(res[1] || ('ubus error ' + res[0]));
		return res[1];
	});
}
var callServiceState = function () { return ubusCall('flclash', 'service_state'); };
var callServiceToggle = function (action) { return ubusCall('flclash', 'service_toggle', { action: action }); };
var callCoreLog = function () { return ubusCall('flclash', 'core_log'); };
var callCoreUpdate = function () { return ubusCall('flclash', 'core_update'); };
var callCoreUpdateStatus = function () { return ubusCall('flclash', 'core_update_status'); };

return view.extend({
	load: function () {
		return L.resolveDefault(callServiceState(), { running: false, enabled: false, webui_url: '' });
	},

	render: function (state) {
		var updating = false;

		/* ---------------------------------------------------------- 服务 tab */

		var statusChip = E('span', { 'class': 'chip' }, '…');
		var coreVersion = E('span', { 'class': 'muted' }, '…');
		var updateLog = E('pre', { 'class': 'flclash-pre', 'style': 'display:none' });
		var toggleBtn = E('button', { 'class': 'cbi-button' }, '…');
		var updateBtn = E('button', { 'class': 'cbi-button' }, _('更新内核'));
		var jumpBtn = E('button', { 'class': 'cbi-button cbi-button-positive' }, _('一键进入 WebUI'));

		function paint(state) {
			var running = !!state.running;
			/* Keep the data flag in sync with the label: the click handler
			 * reads it to choose start vs. stop. Without this, "停止服务"
			 * always issued a start action. */
			toggleBtn.setAttribute('data-running', running ? '1' : '0');
			statusChip.className = 'chip ' + (running ? 'chip-ok' : 'chip-warn');
			statusChip.textContent = running ? _('运行中') : _('已停止');
			toggleBtn.textContent = running ? _('停止服务') : _('启动服务');
			toggleBtn.className = 'cbi-button ' + (running ? 'cbi-button-negative' : 'cbi-button-positive');
		}

		function refreshState() {
			return L.resolveDefault(callServiceState(), null).then(function (next) {
				if (next) paint(next);
				return next;
			});
		}

		var wport = (state && state.webui_port) || '9098';
		var wpath = (state && state.webui_path) || '/flclash/';
		var target = window.location.protocol + '//' + window.location.hostname + ':' + wport + wpath;

		jumpBtn.addEventListener('click', function () {
			callServiceState().then(function (state) {
				if (!state || !state.running) {
					/* The backend is down: bring it up first, then jump. The
					 * proxy core is NOT started here — the user controls it
					 * from the WebUI's own switch. */
					return callServiceToggle('start').then(function () {
						window.open(target, '_blank');
					});
				}
				window.open(target, '_blank');
			});
		});

		toggleBtn.addEventListener('click', function () {
			var running = (toggleBtn.getAttribute('data-running') === '1');
			var action = running ? 'stop' : 'start';
			toggleBtn.disabled = true;
			callServiceToggle(action)
				.then(function (result) {
					if (result && result.code === 0) {
						ui.addNotification(null, E('p', {}, _('FlClash 服务已%s。').format(action == 'start' ? _('启动') : _('停止'))), 'info');
					} else {
						ui.addNotification(null, E('p', {}, _('操作失败(退出码 %s)。').format(result && result.code)), 'error');
					}
				})
				.then(refreshState)
				.finally(function () { toggleBtn.disabled = false; });
		});

		poll.add(refreshState, 3);

		updateBtn.addEventListener('click', function () {
			if (updating) return;
			ui.confirm(_('更新内核'), _('将从上游(mihomo 最新发布版)下载 arm64 内核并自动重启 FlClash 服务。继续吗?'), function () {
				updating = true;
				updateBtn.disabled = true;
				updateLog.style.display = 'block';
				callCoreUpdate().then(function (result) {
					if (result && result.code !== 0) {
						ui.addNotification(null, E('p', {}, _(result.error || '无法启动更新任务')), 'error');
						updating = false;
						updateBtn.disabled = false;
						return;
					}
					poll.add(pollUpdate, 2);
				});
			});
		});

		function pollUpdate() {
			return L.resolveDefault(callCoreUpdateStatus(), null).then(function (status) {
				if (!status) return;
				coreVersion.textContent = status.core_version || '—';
				updateLog.textContent = status.log || '';
				updateLog.scrollTop = updateLog.scrollHeight;
				if (status.state === 'done' || status.state === 'failed') {
					poll.remove(pollUpdate);
					updating = false;
					updateBtn.disabled = false;
					ui.addNotification(null, E('p', {}, status.state === 'done'
						? _('内核更新完成:%s').format(status.core_version || '')
						: _('内核更新失败,详见日志。')), status.state === 'done' ? 'info' : 'error');
				}
			});
		}

		L.resolveDefault(callCoreUpdateStatus(), null).then(function (status) {
			if (status) coreVersion.textContent = status.core_version || '—';
		});

		var svcCard = E('div', { 'class': 'cbi-map' }, [
			E('div', { 'class': 'cbi-section-descr' }, _('这里的开关只控制 WebUI 后端(flclash-api)。代理内核请在 WebUI 内用核心开关手动启停,内核不会随 LuCI 或开机自动拉起。')),
			E('div', { 'class': 'flclash-card' }, [
				E('div', { 'class': 'flclash-row' }, [
					E('span', { 'class': 'flclash-label' }, _('WebUI 后端')),
					statusChip,
					E('span', { 'class': 'flclash-grow' }),
					toggleBtn
				]),
				E('div', { 'class': 'flclash-row' }, [
					E('span', { 'class': 'flclash-label' }, _('内核版本')),
					coreVersion,
					E('span', { 'class': 'flclash-grow' }),
					updateBtn
				]),
				E('div', { 'class': 'flclash-row' }, [
					E('span', { 'class': 'flclash-label' }, _('WebUI')),
					E('span', { 'class': 'muted flclash-url' }, target),
					E('span', { 'class': 'flclash-grow' }),
					jumpBtn
				]),
				updateLog
			])
		]);

		/* ---------------------------------------------------------- 日志 tab */

		var logPre = E('pre', { 'class': 'flclash-pre' }, _('加载中…'));
		var logFilter = E('input', {
			'type': 'text',
			'placeholder': _('过滤关键字,如 fatal'),
			'class': 'cbi-input-text',
			'style': 'margin:0 12px;flex:1'
		});

		function refreshLog() {
			return L.resolveDefault(callCoreLog(), null).then(function (result) {
				var lines = (result && result.lines) || [];
				var keyword = (logFilter.value || '').trim().toLowerCase();
				if (keyword)
					lines = lines.filter(function (line) { return line.toLowerCase().indexOf(keyword) !== -1; });
				logPre.textContent = lines.join('\n') || _('暂无日志');
				logPre.scrollTop = logPre.scrollHeight;
			});
		}
		poll.add(refreshLog, 5);
		refreshLog();

		var logCard = E('div', { 'class': 'cbi-map' }, [
			E('div', { 'class': 'cbi-section-descr' }, _('来自 logread 的 mihomo / flclash 日志,每 5 秒自动刷新。')),
			E('div', { 'style': 'display:flex;align-items:center;margin-bottom:10px' }, [logFilter]),
			logPre
		]);

		/* -------------------------------------------------------------- tabs */

		var tabService = E('button', { 'class': 'flclash-tab is-active', 'type': 'button' }, _('服务'));
		var tabLog = E('button', { 'class': 'flclash-tab', 'type': 'button' }, _('日志'));
		var svcWrap = E('div', {}, [svcCard]);
		var logWrap = E('div', { 'style': 'display:none' }, [logCard]);

		tabService.addEventListener('click', function () {
			tabService.classList.add('is-active');
			tabLog.classList.remove('is-active');
			svcWrap.style.display = '';
			logWrap.style.display = 'none';
			refreshState();
		});
		tabLog.addEventListener('click', function () {
			tabLog.classList.add('is-active');
			tabService.classList.remove('is-active');
			logWrap.style.display = '';
			svcWrap.style.display = 'none';
			refreshLog();
		});

		var tabBar = E('div', { 'class': 'flclash-tabs' }, [tabService, tabLog]);

		paint(state);

		return E([], [E('style', {}, [
			'.flclash-tabs{display:flex;gap:8px;margin-bottom:14px}',
			'.flclash-tab{flex:0 0 auto;padding:8px 22px;border-radius:999px;border:1px solid var(--line,#ddd);background:transparent;font-weight:700;cursor:pointer}',
			'.flclash-tab.is-active{background:#2f7cf6;border-color:#2f7cf6;color:#fff}',
			'.flclash-card{background:var(--card,#fff);border-radius:14px;padding:6px 18px;box-shadow:0 2px 10px rgba(0,0,0,.06)}',
			'.flclash-row{display:flex;align-items:center;gap:12px;padding:12px 0;border-bottom:1px solid rgba(0,0,0,.06)}',
			'.flclash-row:last-child{border-bottom:none}',
			'.flclash-label{font-weight:600;min-width:90px}',
			'.flclash-grow{flex:1}',
			'.flclash-url{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:40%}',
			'.chip{display:inline-flex;padding:3px 12px;border-radius:999px;font-size:12px;font-weight:700;background:#eee}',
			'.chip-ok{background:rgba(47,181,107,.16);color:#2fb56b}',
			'.chip-warn{background:rgba(232,162,59,.16);color:#c07f18}',
			'.flclash-pre{background:#101216;color:#d7e2ee;border-radius:12px;padding:12px 14px;font-size:12px;max-height:300px;overflow:auto;white-space:pre-wrap;word-break:break-all}'
		]), E('h2', {}, _('FlClash')), tabBar, svcWrap, logWrap]);
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
