'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';

/* 短信页：
 *
 *   · 列表：/api/sms/read（文本模式，秒级，直接同步拉）；
 *   · 发送：异步任务 sms.send（长短信要分段写串口，可能十几秒）；
 *   · 删除：POST（快）。
 */

function smsTable(self, list, onDelete) {
	var t = E('table', { 'class': 'table' }, [
		E('tr', { 'class': 'tr table-titles' }, [
			E('th', { 'class': 'th' }, '时间'), E('th', { 'class': 'th' }, '号码'),
			E('th', { 'class': 'th' }, '内容'), E('th', { 'class': 'th' }, '状态'),
			E('th', { 'class': 'th' }, '')
		])
	]);
	(list || []).forEach(function (m) {
		t.appendChild(E('tr', { 'class': 'tr' }, [
			E('td', { 'class': 'td', 'style': 'white-space:nowrap' }, m.timestamp || '-'),
			E('td', { 'class': 'td', 'style': 'white-space:nowrap' }, m.number || '-'),
			E('td', { 'class': 'td' }, (m.content || '') +
				(m.part_total > 1 ? '（' + m.part_seq + '/' + m.part_total + '）' : '')),
			E('td', { 'class': 'td' }, m.status || '-'),
			E('td', { 'class': 'td' }, E('button', {
				'class': 'btn cbi-button cbi-button-remove',
				'click': ui.createHandlerFn(self, function () { return onDelete(m); })
			}, fmt.T('Delete')))
		]));
	});
	if (!(list || []).length) {
		t.appendChild(E('tr', { 'class': 'tr' }, [
			E('td', { 'class': 'td', 'colspan': 5 }, fmt.T('SMS list is empty'))
		]));
	}
	return t;
}

return view.extend({
	load: function () {
		store.start(api);
		var id = store.id() != null ? store.id() : undefined;
		return Promise.all([
			api.get('/api/sms/read', { modem_id: id }),
			api.get('/api/sms/storage', { modem_id: id })
		]);
	},

	render: function (data) {
		var self = this;
		var list = data[0] || [];
		var storage = data[1] || {};
		var id = store.id() != null ? store.id() : 0;

		var listWrap = E('div', {}, [ smsTable(self, list, function (m) {
			return api.post('/api/sms/delete', { modem_id: id, index: m.index }).then(function () {
				ui.addNotification(null, E('p', {}, '已删除'), 'info');
				return self.reload();
			});
		}) ]);

		var form = E('div', { 'class': 'cbi-value' }, [
			E('label', { 'class': 'cbi-value-title' }, fmt.T('Recipient')),
			E('div', { 'class': 'cbi-value-field', 'style': 'display:flex;gap:8px;flex-wrap:wrap' }, [
				E('input', { 'class': 'cbi-input-text', 'id': 'sms-to', 'placeholder': '+86…', 'style': 'width:170px' }),
				E('input', { 'class': 'cbi-input-text', 'id': 'sms-body', 'placeholder': fmt.T('Content'), 'style': 'flex:1;min-width:220px' }),
				E('button', { 'class': 'btn cbi-button cbi-button-apply', 'click': ui.createHandlerFn(self, function (btn) {
					var to = (document.getElementById('sms-to').value || '').trim();
					var body = document.getElementById('sms-body').value || '';
					if (!to || !body) return Promise.reject(new Error('号码和内容都要填'));
					btn.disabled = true;
					var orig = btn.textContent;
					btn.textContent = fmt.T('Submitting…');
					return task.run(api, 'sms.send', { modem_id: id, phone: to, content: body }, { timeout: 90000 })
						.then(function () {
							ui.addNotification(null, E('p', {}, '已发送'), 'info');
							document.getElementById('sms-body').value = '';
							return self.reload();
						})
						.catch(function (e) {
							ui.addNotification(null, E('p', {}, '发送失败：' + (e.message || e)), 'error');
						})
						.finally(function () { btn.disabled = false; btn.textContent = orig; });
				}) }, fmt.T('Send SMS'))
			])
		]);

		return E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '5G 终端 · ' + fmt.T('SMS')),
			E('div', { 'class': 'cbi-map-descr' },
				'发送长短信会自动分段（UCS2），走后台任务，页面不阻塞。'),
			E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, '存储区：' +
					(storage && storage.cur ? storage.cur : '-') +
					(storage && storage.counts ? '（' + storage.counts + '）' : '')),
				listWrap
			]),
			E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, fmt.T('Send SMS')), form
			])
		]);
	},

	reload: function () {
		var id = store.id() != null ? store.id() : undefined;
		return api.get('/api/sms/read', { modem_id: id }).then(function (l) {
			window.location.reload();
		});
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
