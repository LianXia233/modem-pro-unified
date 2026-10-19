'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';
'require modemd.icons as icons';

/* 基站扫描视图（白色毛玻璃架构）
 *
 * 全面按内置 WebUI 对齐：
 *   - 服务小区详情（RAT、Cell ID、PCI、EARFCN、Band、RSRP、RSRQ、SINR、RSSI）
 *   - 邻区基站列表（物理小区 ID、频点、信号质量对比）
 *   - 载波聚合 (CA) 状态与分量载波说明
 *   - 异步任务发起扫描（5~20秒后台采集，带实时计时与可取消保护）
 */

function cellCard(title, cells, isServing) {
	var rows = (cells || []).map(function (c) {
		var grade = fmt.signalGrade(c.rssi, c.rsrp, c.sinr);
		return E('tr', {}, [
			E('td', { 'style': 'font-weight:600' }, [
				isServing ? E('span', { 'class': 'mp-badge mp-badge-primary', 'style': 'margin-right:6px' }, '主服') : '',
				c.tech || '-'
			]),
			E('td', { 'style': 'font-family:ui-monospace,monospace;font-weight:600' }, c.cell_id || '-'),
			E('td', { 'style': 'font-family:ui-monospace,monospace' }, c.pcid != null ? String(c.pcid) : '-'),
			E('td', { 'style': 'font-family:ui-monospace,monospace' }, c.earfcn != null ? String(c.earfcn) : '-'),
			E('td', {}, E('span', { 'class': 'mp-badge mp-badge-muted' }, c.band_label || (c.band ? 'B' + c.band : '-'))),
			E('td', { 'style': 'font-weight:600;color:' + grade.color }, c.rsrp != null ? (c.rsrp + ' dBm') : '-'),
			E('td', {}, c.rsrq != null ? (c.rsrq + ' dB') : '-'),
			E('td', {}, c.sinr != null ? (c.sinr + ' dB') : '-')
		]);
	});

	if (!rows.length) {
		rows = [
			E('tr', {}, [
				E('td', { 'colspan': 8, 'style': 'text-align:center;color:var(--mp-neutral-500);padding:20px' }, '暂无基站小区数据（可点击上方「扫描基站」进行主动探测）')
			])
		];
	}

	return E('div', { 'class': 'mp-card' }, [
		E('div', { 'class': 'mp-card-hd' }, [
			E('h3', { 'class': 'mp-card-title' }, [
				E('span', { 'innerHTML': isServing ? icons.tower(17) : icons.antenna(17) }),
				title + ' (' + (cells ? cells.length : 0) + ' 个)'
			])
		]),
		E('table', { 'class': 'mp-table' }, [
			E('thead', {}, [
				E('tr', {}, [
					E('th', { 'style': 'width:110px' }, '接入制式'),
					E('th', {}, 'Cell ID'),
					E('th', {}, 'PCI (物理小区)'),
					E('th', {}, 'EARFCN (频点)'),
					E('th', {}, '工作频段'),
					E('th', {}, 'RSRP 强度'),
					E('th', {}, 'RSRQ 质量'),
					E('th', {}, 'SINR 信噪比')
				])
			]),
			E('tbody', {}, rows)
		])
	]);
}

return view.extend({
	load: function () {
		fmt.ensureCss();
		store.start(api);
		return store.fetch(api);
	},

	render: function (snap) {
		var self = this;
		snap = snap || {};
		var id = snap.modem_id != null ? snap.modem_id : 0;
		var cells = (snap.blocks && snap.blocks.cells) || {};

		var scanBtn = E('button', {
			'class': 'mp-btn mp-btn-primary',
			'click': ui.createHandlerFn(self, function (btn) {
				btn.disabled = true;
				var orig = btn.innerHTML;
				btn.textContent = '扫描中… 0s';
				return task.run(api, 'cells.scan', { modem_id: id }, {
					timeout: 90000,
					onTick: function (t) {
						btn.textContent = '扫描中… ' + Math.round((t.elapsed_ms || 0) / 1000) + 's';
					}
				}).then(function () {
					ui.addNotification(null, E('p', {}, '基站扫描完成'), 'info');
					store.refresh();
				}).catch(function (e) {
					ui.addNotification(null, E('p', {}, '扫描失败：' + (e.message || e)), 'error');
				}).finally(function () {
					btn.disabled = false;
					btn.innerHTML = orig;
				});
			})
		}, [ E('span', { 'innerHTML': icons.tower(14) }), ' 扫描基站' ]);

		var root = E('div', { 'class': 'mp-wrap' }, [
			fmt.renderTabBar('cells'),

			/* 1. Header */
			E('div', { 'class': 'mp-header' }, [
				E('div', { 'style': 'display:flex;align-items:center;gap:12px;flex-wrap:wrap' }, [
					E('div', { 'class': 'mp-title-badge' }, E('span', { 'innerHTML': icons.tower(20) })),
					E('div', {}, [
						E('h2', { 'class': 'mp-title-text' }, '5G 终端 · ' + fmt.T('Cells')),
						E('div', { 'class': 'mp-subtitle-text' },
							'服务小区、物理小区标识 (PCI)、频点 (EARFCN) 与载波聚合 (CA) 分析')
					])
				]),
				E('div', { 'style': 'display:flex;align-items:center;gap:10px;flex-wrap:wrap' }, [
					E('span', { 'id': 'cells-age-badge', 'class': 'mp-badge mp-badge-muted' }, '未扫描'),
					scanBtn
				])
			]),

			/* 2. 网络概况微卡片 */
			E('div', { 'id': 'cells-meta-card', 'class': 'mp-card', 'style': 'padding:14px 20px' }, [
				E('div', { 'class': 'mp-kv-grid' }, [
					E('div', { 'class': 'mp-kv-item' }, [
						E('span', { 'class': 'mp-kv-label' }, '运营商:'),
						E('span', { 'id': 'cells-op', 'class': 'mp-kv-val' }, cells.operator || '-')
					]),
					E('div', { 'class': 'mp-kv-item' }, [
						E('span', { 'class': 'mp-kv-label' }, '网络接入:'),
						E('span', { 'id': 'cells-netinfo', 'class': 'mp-kv-val' }, cells.net_info || '-')
					]),
					E('div', { 'class': 'mp-kv-item' }, [
						E('span', { 'class': 'mp-kv-label' }, '注册状态:'),
						E('span', { 'id': 'cells-reg', 'class': 'mp-kv-val' }, cells.network_reg || '-')
					])
				])
			]),

			/* 3. 服务小区卡片 */
			E('div', { 'id': 'serving-cells-wrap' }, []),

			/* 4. 邻区基站卡片 */
			E('div', { 'id': 'neighbour-cells-wrap' }, []),

			/* 5. 载波聚合 (CA) 卡片 */
			E('div', { 'id': 'ca-wrap', 'class': 'mp-card' }, [
				E('div', { 'class': 'mp-card-hd' }, [
					E('h3', { 'class': 'mp-card-title' }, [
						E('span', { 'innerHTML': icons.pulse(17) }),
						'载波聚合 (Carrier Aggregation)'
					]),
					E('span', { 'id': 'ca-status-badge', 'class': 'mp-badge mp-badge-muted' }, '未激活')
				]),
				E('div', { 'id': 'ca-body', 'style': 'font-size:13px;color:var(--mp-neutral-700);line-height:1.6' }, [
					'载波聚合状态取决于基站配置与业务下发，数据传输时会自动激活。'
				])
			])
		]);

		this._root = root;
		this.draw(cells, snap);
		store.on(function (s) {
			var snapObj = s.snapshot || {};
			var c = (snapObj.blocks && snapObj.blocks.cells) || {};
			self.draw(c, snapObj);
		});

		return root;
	},

	draw: function (cells, snap) {
		cells = cells || {};
		snap = snap || {};

		/* 数据年龄 */
		var age = (snap.ages || {}).cells;
		var badge = document.getElementById('cells-age-badge');
		if (badge) {
			badge.textContent = age != null ? ('采样于 ' + fmt.ago(age)) : '未扫描';
		}

		/* 顶部概况 */
		var op = document.getElementById('cells-op');
		if (op) op.textContent = cells.operator || '-';
		var ni = document.getElementById('cells-netinfo');
		if (ni) ni.textContent = cells.net_info || '-';
		var reg = document.getElementById('cells-reg');
		if (reg) reg.textContent = cells.network_reg || '-';

		/* 服务小区与邻区 */
		var sWrap = document.getElementById('serving-cells-wrap');
		if (sWrap) {
			sWrap.innerHTML = '';
			sWrap.appendChild(cellCard('服务小区 (Serving Cells)', cells.serving, true));
		}

		var nWrap = document.getElementById('neighbour-cells-wrap');
		if (nWrap) {
			nWrap.innerHTML = '';
			nWrap.appendChild(cellCard('邻区基站 (Neighbour Cells)', cells.neighbours, false));
		}

		/* CA */
		var caBadge = document.getElementById('ca-status-badge');
		if (caBadge) {
			caBadge.textContent = cells.ca_active ? '已激活' : '未激活 / 单载波';
			caBadge.className = 'mp-badge ' + (cells.ca_active ? 'mp-badge-success' : 'mp-badge-muted');
		}

		var caBody = document.getElementById('ca-body');
		if (caBody) {
			caBody.innerHTML = '';
			if (cells.ca_note) {
				caBody.appendChild(E('p', { 'style': 'margin:0 0 6px;white-space:pre-wrap' }, cells.ca_note));
			}
			if (cells.ca_cmds && cells.ca_cmds.length) {
				caBody.appendChild(E('div', { 'class': 'mp-code-box', 'style': 'margin-top:6px' },
					'查询指令：' + cells.ca_cmds.join(' / ')));
			}
		}
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
