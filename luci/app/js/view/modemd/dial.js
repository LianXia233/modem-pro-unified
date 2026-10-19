'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';

/* 拨号上网页：
 *
 *   · 状态/地址：/api/dial（本地状态为主，usb 识别有 30s 缓存）；
 *   · 连接/断开/开关：直接 POST（快接口，modemd 内部立刻回执）；
 *   · APN 列表与修改：/api/params（快）；写操作 POST 后刷新。
 */

function row(k, v) {
	return E('tr', { 'class': 'tr' }, [
		E('td', { 'class': 'td', 'style': 'width:180px;font-weight:bold' }, k),
		E('td', { 'class': 'td' }, v == null || v === '' ? '-' : String(v))
	]);
}

return view.extend({
	load: function () {
		store.start(api);
		return Promise.all([
			store.fetch(api),
			api.get('/api/dial', { modem_id: store.id() != null ? store.id() : undefined }),
			api.get('/api/params', { modem_id: store.id() != null ? store.id() : undefined })
		]);
	},

	render: function (data) {
		var self = this;
		var snap = data[0] || {};
		var dial = data[1] || {};
		var params = data[2] || {};
		var st = dial.state || {};
		var id = dial.modem_id != null ? dial.modem_id : (snap.modem_id != null ? snap.modem_id : 0);

		/* 状态表 */
		var tbl = E('table', { 'class': 'table' }, [
			row('状态', st.state_text || st.state),
			row('接口类型', st.type_name || st.kind),
			row('主机侧网卡', st.host_iface),
			row('IPv4', st.ipv4),
			row('IPv6', st.ipv6),
			row('APN', st.apn || '（模组默认）'),
			row('CID', st.cid),
			row('在线时长', st.elapsed_sec ? Math.round(st.elapsed_sec / 60) + ' min' : '-')
		]);

		/* 自动组网最近一次结果 */
		var auto = dial.auto_wan;
		var autoTbl = E('table', { 'class': 'table' });
		if (auto && auto.cycle != null) {
			autoTbl.appendChild(row('最近一轮', '#' + auto.cycle + ' · ' + (auto.changed ? '有变更' : '无变更')));
			if (auto.note) autoTbl.appendChild(row('说明', auto.note));
		} else {
			autoTbl.appendChild(row('最近一轮', '尚未运行（组网循环每 15 秒一轮）'));
		}

		/* 操作按钮 */
		var actions = E('div', { 'class': 'cbi-section-actions', 'style': 'display:flex;flex-wrap:wrap;gap:8px' }, [
			E('button', { 'class': 'btn cbi-button cbi-button-apply', 'click': ui.createHandlerFn(self, function () {
				return api.post('/api/dial/connect', { modem_id: id }).then(function (d) {
					ui.addNotification(null, E('p', {}, (d && d.msg) || '已交办，等下一轮组网循环拉起'), 'info');
					store.refresh();
				}).catch(function (e) { ui.addNotification(null, E('p', {}, String(e.message || e)), 'error'); });
			}) }, '连接'),
			E('button', { 'class': 'btn cbi-button cbi-button-reset', 'click': ui.createHandlerFn(self, function () {
				return api.post('/api/dial/disconnect', { modem_id: id }).then(function (d) {
					ui.addNotification(null, E('p', {}, (d && d.msg) || '已断开'), 'info');
					store.refresh();
				}).catch(function (e) { ui.addNotification(null, E('p', {}, String(e.message || e)), 'error'); });
			}) }, '断开'),
			E('button', { 'class': 'btn cbi-button', 'click': ui.createHandlerFn(self, function (btn) {
				return api.post('/api/dial/enable', { modem_id: id, enabled: !st.dial_enabled }).then(function () {
					ui.addNotification(null, E('p', {}, st.dial_enabled ? '拨号已关闭' : '拨号已打开'), 'info');
					return self.reloadDial();
				});
			}) }, st.dial_enabled ? '关闭拨号' : '打开拨号'),
			E('button', { 'class': 'btn cbi-button', 'click': ui.createHandlerFn(self, function () {
				return api.post('/api/dial/ipv6', { modem_id: id, enabled: !dial.wan_ipv6 }).then(function () {
					ui.addNotification(null, E('p', {}, dial.wan_ipv6 ? 'IPv6 已关闭' : 'IPv6 已打开'), 'info');
					return self.reloadDial();
				});
			}) }, dial.wan_ipv6 ? '关闭 IPv6' : '打开 IPv6')
		]);

		/* APN 列表 */
		var apns = (params && params.apns) || [];
		var apnTbl = E('table', { 'class': 'table' }, [
			E('tr', { 'class': 'tr table-titles' }, [
				E('th', { 'class': 'th' }, 'CID'), E('th', { 'class': 'th' }, 'APN'),
				E('th', { 'class': 'th' }, '用户'), E('th', { 'class': 'th' }, 'PDP'),
				E('th', { 'class': 'th' }, '默认'), E('th', { 'class': 'th' }, '')
			])
		]);
		apns.forEach(function (a) {
			apnTbl.appendChild(E('tr', { 'class': 'tr' }, [
				E('td', { 'class': 'td' }, String(a.cid)),
				E('td', { 'class': 'td' }, a.apn || '-'),
				E('td', { 'class': 'td' }, a.user || '-'),
				E('td', { 'class': 'td' }, a.pdp_type || '-'),
				E('td', { 'class': 'td' }, a.default_cid ? '✓' : ''),
				E('td', { 'class': 'td' }, E('button', { 'class': 'btn cbi-button cbi-button-remove', 'click': ui.createHandlerFn(self, function () {
					return api.post('/api/params/apn/delete', { modem_id: id, cid: a.cid }).then(function () {
						ui.addNotification(null, E('p', {}, '已删除'), 'info');
						return self.reloadDial();
					});
				}) }, fmt.T('Delete')))
			]));
		});
		if (!apns.length) {
			apnTbl.appendChild(E('tr', { 'class': 'tr' }, [
				E('td', { 'class': 'td', 'colspan': 6 }, '（模组里没有保存的 APN）')
			]));
		}

		/* 新增 APN */
		var apnForm = E('div', { 'class': 'cbi-value' }, [
			E('label', { 'class': 'cbi-value-title' }, fmt.T('APN')),
			E('div', { 'class': 'cbi-value-field', 'style': 'display:flex;gap:8px;flex-wrap:wrap' }, [
				E('input', { 'class': 'cbi-input-text', 'id': 'modemd-apn', 'placeholder': 'APN（如 ctnet）', 'style': 'width:180px' }),
				E('input', { 'class': 'cbi-input-text', 'id': 'modemd-apn-user', 'placeholder': '用户（可空）', 'style': 'width:120px' }),
				E('input', { 'class': 'cbi-input-text', 'id': 'modemd-apn-pass', 'placeholder': '密码（可空）', 'type': 'password', 'style': 'width:120px' }),
				E('button', { 'class': 'btn cbi-button cbi-button-apply', 'click': ui.createHandlerFn(self, function () {
					var apn = (document.getElementById('modemd-apn').value || '').trim();
					if (!apn) return Promise.reject(new Error('APN 不能为空'));
					return api.post('/api/params/apn', {
						modem_id: id, apn: apn,
						user: (document.getElementById('modemd-apn-user').value || '').trim(),
						pass: (document.getElementById('modemd-apn-pass').value || '').trim()
					}).then(function () {
						ui.addNotification(null, E('p', {}, 'APN 已保存'), 'info');
						return self.reloadDial();
					});
				}) }, '保存 APN')
			])
		]);

		var root = E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '5G 终端 · ' + fmt.T('Dial-up')),
			E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, fmt.T('Status')),
				tbl, E('div', { 'style': 'height:8px' }), actions
			]),
			E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, '自动组网'), autoTbl
			]),
			E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, 'APN 配置'), apnTbl, apnForm
			])
		]);

		this._root = root;
		return root;
	},

	/* 重新拉一次拨号/参数（快接口）并就地刷新状态表 */
	reloadDial: function () {
		var self = this;
		return Promise.all([
			api.get('/api/dial', { modem_id: store.id() != null ? store.id() : undefined }),
			api.get('/api/params', { modem_id: store.id() != null ? store.id() : undefined })
		]).then(function () {
			store.refresh();
		});
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
