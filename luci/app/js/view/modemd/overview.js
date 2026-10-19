'use strict';
'require view';
'require poll';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';

/* 设备中心（异步版，2026-10-19 起取代 iframe 嵌入）：
 *
 * 数据只来自 /api/snapshot（缓存 + 年龄），**打开本页一条 AT 都不发**。
 * 需要新数据时点「刷新」→ 提交异步任务（后端工作线程去读模组）→ 任务完成后
 * 快照轮询自然带回新值。页面在轮询间隙完全空闲，多个标签页共享一条轮询
 * （见 modemd/store.js）。
 */

function card(title, bodyId, extra) {
	return E('div', { 'class': 'cbi-section', 'id': 'card-' + bodyId }, [
		E('div', { 'class': 'cbi-section-title', 'style': 'display:flex;align-items:center;gap:8px' }, [
			title,
			extra || E('span', {}, [])
		]),
		E('div', { 'id': bodyId }, [ fmt.T('Reading') ])
	]);
}

function kv(k, v) {
	return E('div', { 'class': 'cbi-value' }, [
		E('label', { 'class': 'cbi-value-title' }, k),
		E('div', { 'class': 'cbi-value-field' }, v == null ? '-' : String(v))
	]);
}

return view.extend({
	load: function () {
		return Promise.all([
			api.get('/api/version'),
			store.start(api)
		]);
	},

	render: function () {
		var self = this;

		function taskBtn(kind, label) {
			return E('button', {
				'class': 'btn cbi-button cbi-button-apply',
				'click': ui.createHandlerFn(self, function (btn) {
					btn.disabled = true;
					var orig = btn.textContent;
					btn.textContent = fmt.T('Submitting…');
					task.run(api, kind, {}, {}).then(function () {
						ui.addNotification(null, E('p', {}, label + ' — ' + fmt.T('Done')), 'info');
						store.refresh();
					}).catch(function (e) {
						ui.addNotification(null, E('p', {}, label + ' — ' + (e.message || e)), 'error');
					}).finally(function () {
						btn.disabled = false;
						btn.textContent = orig;
					});
				})
			}, label);
		}

		var root = E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '5G 终端 · ' + fmt.T('Device Center')),
			E('div', { 'class': 'cbi-map-descr' },
				'页面显示的是最近一次采样值（标注了数据年龄）；点「刷新」会提交后台任务读取模组，完成后自动更新，全程不阻塞页面。'),

			E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' }, fmt.T('Select modem')),
				E('div', { 'class': 'cbi-value-field' },
					E('select', { 'id': 'modemd-sel', 'style': 'min-width:260px', 'change': ui.createHandlerFn(self, function (ev) {
						store.select(parseInt(ev.target.value, 10));
					}) }, []))
			]),

			E('div', { 'id': 'modemd-tasks', 'style': 'margin:8px 0' }, []),

			card(fmt.T('Signal'), 'ov-signal', taskBtn('signal.refresh', fmt.T('Refresh signal'))),
			card(fmt.T('Network'), 'ov-network', taskBtn('network.refresh', fmt.T('Refresh network'))),
			card(fmt.T('Cells'), 'ov-cells',
				E('a', { 'href': LuCI.url('admin/modemd/cells') }, '→ ' + fmt.T('Cells'))),
			card(fmt.T('Identity'), 'ov-identity'),
			card(fmt.T('Dial-up'), 'ov-dial',
				E('a', { 'href': LuCI.url('admin/modemd/dial') }, '→ ' + fmt.T('Dial-up')))
		]);

		store.on(function (s) { self.redraw(s); });
		return root;
	},

	redraw: function (s) {
		var self = this;

		/* 模组下拉 */
		var sel = document.getElementById('modemd-sel');
		if (sel) {
			var opts = (s.modems || []).map(function (m) {
				return E('option', { 'value': String(m.id) },
					'#' + m.id + ' ' + (m.model || m.name || 'modem') +
					(m.running ? '' : ' (' + fmt.T('Failed') + ')'));
			});
			sel.innerHTML = '';
			if (!opts.length) {
				sel.appendChild(E('option', {}, fmt.T('No modem detected')));
				sel.disabled = true;
			} else {
				opts.forEach(function (o) { sel.appendChild(o); });
				sel.disabled = false;
				if (s.modemId != null) sel.value = String(s.modemId);
			}
		}

		/* 任务条 */
		var tdiv = document.getElementById('modemd-tasks');
		if (tdiv) {
			var tasks = (s.snapshot && s.snapshot.tasks) || [];
			tdiv.innerHTML = '';
			tasks.forEach(function (t) {
				tdiv.appendChild(E('div', { 'class': 'alert-message info', 'style': 'display:flex;gap:10px;align-items:center' }, [
					E('span', {}, '[' + fmt.T(t.state) + '] ' + t.label + (t.progress ? ' · ' + t.progress : '') + ' · ' + (t.elapsed_ms / 1000).toFixed(0) + 's'),
					E('button', { 'class': 'btn cbi-button cbi-button-remove', 'click': ui.createHandlerFn(self, function () {
						api.taskCancel(t.id).then(function () { store.refresh(); });
					}) }, fmt.T('Cancel'))
				]));
			});
		}

		var snap = s.snapshot;
		if (!snap) {
			var em = document.getElementById('ov-signal');
			if (em && s.err) { em.innerHTML = ''; em.appendChild(E('p', { 'class': 'alert-message warning' }, s.err)); }
			return;
		}
		var id = snap.modem_id;
		var B = snap.blocks || {};
		var sig = B.signal, net = B.network, cells = B.cells;

		function setBody(elId, nodes) {
			var el = document.getElementById(elId);
			if (el) { el.innerHTML = ''; (nodes || []).forEach(function (n) { el.appendChild(n); }); }
		}

		/* 信号 */
		var sigRows = [];
		if (sig) {
			sigRows.push(kv('RSSI', fmt.rssiText(sig.rssi) + ' (' + fmt.bars(sig.rssi) + '/5)'));
			sigRows.push(kv(fmt.T('RSRP'), fmt.dash(sig.rsrp != null ? sig.rsrp + ' dBm' : null)));
			sigRows.push(kv(fmt.T('RSRQ'), fmt.dash(sig.rsrq != null ? sig.rsrq + ' dB' : null)));
			sigRows.push(kv(fmt.T('SINR'), fmt.dash(sig.sinr != null ? sig.sinr + ' dB' : null)));
			this.ageRow(sigRows, snap, 'signal');
		}
		setBody('ov-signal', sigRows.length ? sigRows : [E('p', { 'class': 'muted' }, fmt.T('Reading'))]);

		/* 网络 */
		var netRows = [];
		if (net) {
			netRows.push(kv(fmt.T('Operator'), fmt.dash(net.operator_name || net.operator)));
			netRows.push(kv(fmt.T('Band'), fmt.dash(net.band_label || (net.band > 0 ? 'B' + net.band : null))));
			netRows.push(kv(fmt.T('Status'), fmt.regText(net.ps_reg) +
				(net.roaming ? ' · ' + fmt.T('Roaming') : '') +
				(net.airplane ? ' · ' + fmt.T('Airplane mode') : '')));
			netRows.push(kv(fmt.T('APN'), fmt.dash(net.apn)));
			if (net.ipv4) netRows.push(kv(fmt.T('IPv4'), fmt.dash(net.ipv4)));
			if (net.ipv6) netRows.push(kv(fmt.T('IPv6'), fmt.dash(net.ipv6)));
			this.ageRow(netRows, snap, 'network');
		}
		setBody('ov-network', netRows.length ? netRows : [E('p', { 'class': 'muted' }, fmt.T('Reading'))]);

		/* 小区摘要 */
		var cellRows = [];
		if (cells) {
			cellRows.push(kv(fmt.T('Serving cell'), String(cells.serving_count || 0)));
			cellRows.push(kv(fmt.T('Neighbour cells'), String(cells.neighbour_count || 0)));
			this.ageRow(cellRows, snap, 'cells');
		}
		setBody('ov-cells', cellRows.length ? cellRows : [E('p', { 'class': 'muted' }, fmt.T('Reading'))]);

		/* 身份 */
		var mod = (s.modems || []).filter(function (m) { return m.id === id; })[0];
		var idRows = [];
		if (mod) {
			idRows.push(kv('Model', fmt.dash(mod.model)));
			idRows.push(kv('Vendor', fmt.dash(mod.vendor_name || mod.vendor)));
			idRows.push(kv('IMEI', fmt.dash(mod.imei)));
			idRows.push(kv('ICCID', fmt.dash(mod.iccid)));
			idRows.push(kv('SIM', fmt.dash(mod.sim_state)));
			idRows.push(kv('AT', fmt.dash(mod.at_port) + (mod.at_port_manual ? ' (manual)' : '')));
		}
		setBody('ov-identity', idRows.length ? idRows : [E('p', { 'class': 'muted' }, fmt.T('Reading'))]);

		/* 拨号摘要 */
		var dialRows = [];
		if (snap.dial && snap.dial.state) {
			dialRows.push(kv(fmt.T('Status'), fmt.dash(snap.dial.state.state)));
			dialRows.push(kv(fmt.T('IPv4'), fmt.dash(snap.dial.state.ipv4)));
			dialRows.push(kv(fmt.T('Interface'), fmt.dash(snap.dial.state.iface)));
		}
		setBody('ov-dial', dialRows.length ? dialRows : [E('p', { 'class': 'muted' }, '-')]);
	},

	/* 「数据 N 秒旧」一行 */
	ageRow: function (rows, snap, kind) {
		var age = (snap.ages || {})[kind];
		var isStale = (snap.stale || {})[kind];
		var text = fmt.T('Data is %s old').replace('%s', fmt.ago(age));
		rows.push(E('div', { 'class': 'cbi-value' }, [
			E('label', { 'class': 'cbi-value-title' }, ' '),
			E('div', { 'class': 'cbi-value-field' },
				E('em', { 'style': isStale ? 'color:#b45f06' : 'color:#4a7a4a' }, text))
		]));
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
