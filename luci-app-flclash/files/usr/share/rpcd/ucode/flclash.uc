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
 *   core_update         (write) spawn core-update.sh with a core argument
 *   core_update_status  (read)  { state, log, core_version, core_type }
 *   core_type           (read)  active core + installed binaries
 *   core_switch         (write) persist the selected core (mihomo|sing-box)
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
	/* The LuCI switch manages the WebUI backend (flclash-api) only. The
	 * proxy core is intentionally NOT touched here: the user starts/stops
	 * it from the WebUI's own switch, so LuCI can never silently turn the
	 * transparent proxy on or off. */
	const rc = system('/etc/init.d/flclash-api status >/dev/null 2>&1');
	const running = (rc == 0);
	const enabled = (system('[ -e /etc/rc.d/S*flclash-api ]') == 0);
	return { running, enabled };
}

function update_state() {
	const raw = readfile('/tmp/flclash-update.state');
	return (raw == null) ? 'idle' : replace(trim(raw), '\n', '');
}

function active_core_type() {
	const all = get_all('flclash');
	const main = all.main || {};
	let type = main.core_type || 'mihomo';
	if (type != 'mihomo' && type != 'sing-box')
		type = 'mihomo';
	return type;
}

/* Resolve a core's version string without invoking the binary directly:
 *  - sing-box exposes a standard `version` subcommand;
 *  - the bundled mihomo is the native-IPC build whose `-v` flag is parsed
 *    as a socket path and panics, so read the version from the local API
 *    bridge (/flclash/api/version) instead. Results are cached briefly in
 *    /tmp so the value stays visible even if the controller is restarting.
 */
function core_version(type) {
	let v = '';
	if (type == 'sing-box') {
		/* "sing-box version 1.2.3" -> third whitespace-separated field. */
		const raw = trim(run('/usr/bin/sing-box version 2>/dev/null | head -n1'));
		const parts = split(raw, ' ');
		v = (length(parts) >= 3) ? parts[2] : '';
	} else {
		/* {"meta":true,"version":"1.2.3"} -> value after "version":" */
		const raw = trim(run('wget -q -O - --timeout=3 http://127.0.0.1:9098/flclash/api/version 2>/dev/null'));
		const q = '"';
		const key = 'version' + q + ':' + q;
		const p = index(raw, key);
		if (p >= 0) {
			const rest = substr(raw, p + length(key));
			const e = index(rest, q);
			if (e > 0)
				v = substr(rest, 0, e);
		}
	}
	if (length(v) > 0) {
		system('echo "' + v + '" > /tmp/flclash-core-version.' + type);
		return v;
	}
	const cached = readfile('/tmp/flclash-core-version.' + type);
	return (cached == null) ? '' : trim(cached);
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
		/* On this rpcd-ucode build declared args arrive under req.args. */
		call: function (request) {
			const action = (request?.args?.action != null) ? request.args.action : null;
			if (action != 'start' && action != 'stop' && action != 'restart')
				return { code: 1, error: '未知操作' };

			const rc = system('/etc/init.d/flclash-api ' + action);
			/* Give procd a moment so the caller reads the fresh state. */
			system('sleep 1');
			const state = core_state();
			return { code: rc, running: state.running, action: action };
		},
		args: { action: 'start | stop | restart' }
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
		call: function (request) {
			let core = (request?.args?.core != null) ? request.args.core : 'mihomo';
			if (core != 'mihomo' && core != 'sing-box')
				return { code: 1, error: '未知内核类型' };

			const state = update_state();
			if (state == 'running')
				return { code: 1, error: '已有更新任务在执行' };

			system('echo running > /tmp/flclash-update.state; : > /tmp/flclash-update.log');
			system('/usr/libexec/flclash/core-update.sh ' + core + ' >/tmp/flclash-update.log 2>&1 &');
			return { code: 0, state: 'running', core: core };
		},
		args: { core: 'mihomo | sing-box' }
	},

	core_update_status: {
		call: function () {
			const state = update_state();
			const log = readfile('/tmp/flclash-update.log');
			const type = active_core_type();
			const ver = core_version(type);
			return {
				state: state,
				log: (log == null) ? '' : log,
				core_version: ver,
				core_type: type
			};
		}
	},

	core_type: {
		call: function () {
			return {
				active: active_core_type(),
				mihomo_installed: (system('[ -x /usr/bin/mihomo ]') == 0),
				singbox_installed: (system('[ -x /usr/bin/sing-box ]') == 0),
				mihomo_version: core_version('mihomo'),
				singbox_version: core_version('sing-box')
			};
		}
	},

	core_switch: {
		call: function (request) {
			const core = (request?.args?.core != null) ? request.args.core : null;
			if (core != 'mihomo' && core != 'sing-box')
				return { code: 1, error: '未知内核类型' };
			const bin = (core == 'sing-box') ? '/usr/bin/sing-box' : '/usr/bin/mihomo';
			if (system('[ -x ' + bin + ' ]') != 0)
				return { code: 1, error: '内核未安装,请先在线更新' };

			system('uci -q set flclash.main.core_type=' + core);
			// Switching back to the bundled mihomo restores native IPC mode; the
			// online updater sets core_mode=cli only when it installs the
			// standalone CLI mihomo. sing-box ignores core_mode.
			if (core == 'mihomo')
				system('uci -q set flclash.main.core_mode=ipc');
			system('uci -q commit flclash');

			// Lossless subscription conversion: before starting sing-box,
			// regenerate its native JSON from the current mihomo config.yaml.
			// The init script does this too, but doing it here guarantees the
			// switch sees freshly-converted proxies/groups/rules/DNS/TUN.
			let converted = false;
			if (core == 'sing-box') {
				converted = (system('/usr/libexec/flclash/singbox-convert.sh /etc/flclash/config.yaml /tmp/flclash-singbox.json >/dev/null 2>&1') == 0);
			}
			system('/etc/init.d/flclash-core restart >/dev/null 2>&1');
			return { code: 0, core: core, converted: converted, running: core_state().running };
		},
		args: { core: 'mihomo | sing-box' }
	}
};

return { flclash: methods };
