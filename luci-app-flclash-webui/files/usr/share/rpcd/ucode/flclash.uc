/* rpcd ubus object: flclash
 *
 * Thin adapter for the LuCI app. Read methods never touch the core;
 * write methods funnel through the init script and the libexec updater so
 * the ACL governs them as a unit. ucode only — no external helpers.
 *
 * Methods:
 *   system_proxy        (read)  legacy read-only summary for the WebUI
 *   service_state       (read)  { running, enabled } of flclash-core
 *   service_toggle      (write) { action: start|stop|restart } via init.d
 *   core_log            (read)  recent mihomo/flclash lines from logread
 *   core_update         (write) spawn /usr/libexec/flclash/core-update.sh
 *   core_update_status  (read)  { state, log, core_version }
 */
'use strict';

import { readfile, popen } from 'fs';
import { cursor } from 'uci';

let uci_ctx = null;

function get_all(cfg) {
	if (uci_ctx == null)
		uci_ctx = cursor();
	const res = uci_ctx.get_all(cfg);
	return (res == null) ? {} : res;
}

function run(cmd) {
	const out = popen(cmd);
	if (out == null)
		return '';
	let data = '';
	for (;;) {
		const chunk = out.read(4096);
		if (chunk == null || length(chunk) == 0)
			break;
		data = data + chunk;
	}
	out.close();
	return data;
}

function core_state() {
	/* rc.common `status` exits 0 when the procd instance is running. */
	const rc = system('/etc/init.d/flclash-core status >/dev/null 2>&1');
	const running = (rc == 0);
	const enabled = (system('[ -e /etc/rc.d/S*flclash-core ]') == 0);
	return { running, enabled };
}

function update_state() {
	const raw = readfile('/tmp/flclash-update.state');
	return (raw == null) ? 'idle' : replace(trim(raw), '\n', '');
}

const methods = {
	system_proxy: {
		call: function () {
			const all = get_all('flclash');
			const main = all.main || {};
			const webui = all.webui || {};
			const running_process = (match(run('ps w'), /(mihomo|sing-box|clash)/) != null);
			return {
				backend: 'flclash',
				mixed_port: int(main.mixed_port) || 0,
				clash_api_port: int(main.clash_api_port) || 0,
				tun_name: main.tun_name || '',
				routing_mode: main.routing_mode || '',
				ipv6_support: (main.ipv6 == '1'),
				webui_port: int(webui.webui_port) || 9098,
				running: running_process,
				read_only: true,
				source: 'device-uci-ps'
			};
		}
	},

	service_state: {
		call: function () {
			const state = core_state();
			const all = get_all('flclash');
			const webui = all.webui || {};
			const main = all.main || {};
			let port = int(webui.webui_port) || 9098;
			let path = webui.webui_path || '/flclash/';
			if (substr(path, 0, 1) != '/')
				path = '/' + path;
			if (substr(path, length(path) - 1) != '/')
				path = path + '/';
			return {
				running: state.running,
				enabled: state.enabled,
				webui_port: port,
				webui_path: path,
				mixed_port: int(main.mixed_port) || 0
			};
		}
	},

	service_toggle: {
		call: function (request) {
			const action = (request != null) ? request.action : null;
			if (action != 'start' && action != 'stop' && action != 'restart')
				return { code: 1, error: '未知操作' };

			const rc = system('/etc/init.d/flclash-core ' + action);
			/* Give procd a moment so the caller reads the fresh state. */
			system('sleep 1');
			const state = core_state();
			return { code: rc, running: state.running, action: action };
		}
	},

	core_log: {
		call: function () {
			const raw = run('logread | grep -E "mihomo|flclash" | tail -120');
			const lines = split(raw, '\n');
			const out = [];
			for (let line in lines) {
				if (length(trim(line)) > 0)
					push(out, trim(line));
			}
			return { lines: out };
		}
	},

	core_update: {
		call: function () {
			const state = update_state();
			if (state == 'running')
				return { code: 1, error: '已有更新任务在执行' };

			system('echo running > /tmp/flclash-update.state; : > /tmp/flclash-update.log');
			system('/usr/libexec/flclash/core-update.sh >/tmp/flclash-update.log 2>&1 &');
			return { code: 0, state: 'running' };
		}
	},

	core_update_status: {
		call: function () {
			const state = update_state();
			const log = readfile('/tmp/flclash-update.log');
			return {
				state: state,
				log: (log == null) ? '' : log,
				core_version: trim(run('/usr/bin/mihomo -v 2>/dev/null | head -n1'))
			};
		}
	}
};

return { flclash: methods };
