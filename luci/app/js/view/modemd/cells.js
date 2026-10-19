'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';

/* 基站扫描页：
 *
 * 扫描是慢操作（5~20 秒独占串口），**只走异步任务**：提交 → 立即返回 →
 * 轮询进度；期间页面可以随便切别的页，任务完成再回来也能看到结果
 * （结果缓存在后端，快照直接带回）。
 */

function cellTable(cells, title) {
	var t = E('table', { 'class': 'table' }, [
		E('tr', { 'class': 'tr table-titles' }, [
			E('th', { 'class': 'th' }, 'RAT'), E('th', { 'class': 'th' }, 'Cell ID'),
			E('th', { 'class': 'th' }, 'PCI'), E('th', { 'class': 'th' }, 'EARFCN'),
			E('th', { 'class': 'th' }, 'Band'), E('th', { 'class': 'th' }, 'RSRP'),
			E('th', { 'class': 'th' }, 'RSRQ'), E('th', { 'class': 'th' }, 'SINR')
		])
	]);
	(cells || []).forEach(function (c) {
		t.appendChild(E('tr', { 'class': 'tr' + (c.serving ? ' cbi-rowstyle-1' : '') }, [
			E('td', { 'class': 'td' }, (c.serving ? '★ ' : '') + (c.tech || '-')),
			E('td', { 'class': 'td' }, c.cell_id || '-'),
			E('td', { 'class': 'td' }, c.pcid || '-'),
			E('td', { 'class': 'td' }, c.earfcn || '-'),
			E('td', { 'class': 'td' }, c.band_label || (c.band || '-')),
			E('td', { 'class': 'td' }, c.rsrp + ' dBm'),
			E('td', { 'class': 'td' }, c.rsrq + ' dB'),
			E('td', { 'class': 'td' }, c.sinr + ' dB')
		]));
	});
	if (!(cells || []).length) {
		t.appendChild(E('tr', { 'class': 'tr' }, [
			E('td', { 'class': 'td', 'colspan': 8 }, '（无）')
		]));
	}
	return E('div', { 'class': 'cbi-section' }, [
		E('div', { 'class': 'cbi-section-title' }, title), t
	]);
}

return view.extend({
	load: function () {
		store.start(api);
		return store.fetch(api);
	},

	render: function (snap) {
		var self = this;
		var id = snap.modem_id != null ? snap.modem_id : 0;
		var cells = (snap.blocks && snap.blocks.cells) || null;

		var info = E('div', { 'class': 'cbi-section' }, []);
		var body = E('div', {}, []);

		var scanBtn = E('button', {
			'class': 'btn cbi-button cbi-button-apply',
			'click': ui.createHandlerFn(self, function (btn) {
				btn.disabled = true;
				var orig = btn.textContent;
				btn.textContent = '扫描中…';
				return task.run(api, 'cells.scan', { modem_id: id }, {
					timeout: 90000,
					onTick: function (t) {
						btn.textContent = '扫描中… ' + Math.round((t.elapsed_ms || 0) / 1000) + 's';
					}
				}).then(function () {
					ui.addNotification(null, E('p', {}, '扫描完成'), 'info');
					store.refresh();
				}).catch(function (e) {
					ui.addNotification(null, E('p', {}, '扫描失败：' + (e.message || e)), 'error');
				}).finally(function () {
					btn.disabled = false;
					btn.textContent = orig;
				});
			})
		}, fmt.T('Scan cells'));

		var root = E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '5G 终端 · ' + fmt.T('Cells')),
			E('div', { 'class': 'cbi-map-descr' },
				'扫描会独占模组串口 5~20 秒（期间信号/网络查询会排队），所以这里走后台任务：点「扫描」后页面立刻可以干别的，完成后自动刷新。'),
			E('div', { 'class': 'cbi-section-actions', 'style': 'display:flex;gap:8px;align-items:center' }, [
				scanBtn,
				E('span', { 'id': 'cells-age', 'class': 'muted' }, '')
			]),
			body
		]);

		/* 首帧 + 订阅更新 */
		this.draw(body, info, cells, snap);
		store.on(function (s) {
			self.draw(body, info, s.snapshot && s.snapshot.blocks ? s.snapshot.blocks.cells : null, s.snapshot || {});
			var age = s.snapshot && s.snapshot.ages ? s.snapshot.ages.cells : null;
			var el = document.getElementById('cells-age');
			if (el) el.textContent = age != null ? ('数据 ' + fmt.ago(age) + ' 前采集') : '';
		});
		return root;
	},

	draw: function (body, info, cells, snap) {
		body.innerHTML = '';
		if (!cells) {
			body.appendChild(E('p', { 'class': 'muted' }, '还没有扫描结果：点上面的「扫描基站」开始。'));
			return;
		}
		if (cells.operator || cells.network_reg || cells.net_info) {
			info.innerHTML = '';
			var t = E('table', { 'class': 'table' });
			if (cells.operator) t.appendChild(E('tr', { 'class': 'tr' }, [
				E('td', { 'class': 'td', 'style': 'width:160px;font-weight:bold' }, '运营商'),
				E('td', { 'class': 'td' }, cells.operator)]));
			if (cells.net_info) t.appendChild(E('tr', { 'class': 'tr' }, [
				E('td', { 'class': 'td', 'style': 'width:160px;font-weight:bold' }, '网络接入信息'),
				E('td', { 'class': 'td' }, cells.net_info)]));
			if (cells.network_reg) t.appendChild(E('tr', { 'class': 'tr' }, [
				E('td', { 'class': 'td', 'style': 'width:160px;font-weight:bold' }, '注册状态'),
				E('td', { 'class': 'td' }, cells.network_reg)]));
			info.appendChild(t);
			body.appendChild(info);
		}
		body.appendChild(cellTable(cells.serving, fmt.T('Serving cell')));
		body.appendChild(cellTable(cells.neighbours, fmt.T('Neighbour cells')));
		if (cells.ca_note) {
			body.appendChild(E('p', { 'class': 'muted', 'style': 'white-space:pre-line' },
				'载波聚合：' + cells.ca_note +
				((cells.ca_cmds && cells.ca_cmds.length) ? '（查询指令：' + cells.ca_cmds.join(' / ') + '）' : '')));
		}
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
