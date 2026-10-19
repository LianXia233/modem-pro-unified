'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';
'require modemd.icons as icons';

/* 锁频段 / 锁基站 / 组网偏好视图（白色毛玻璃架构）
 *
 * 全面按内置 WebUI 对齐：
 *   - 锁网状态概览（LTE/NR 当前锁定频段、设备能力、清单支持对照、方言识别）
 *   - 锁频段（LTE / NR / NSA / SA 分组勾选芯片、下发、解除单个制式、一键全部解锁）
 *   - 锁小区（锁频点+PCI / 仅锁频点 / 解除锁定，EARFCN、PCI、SCS）
 *   - 组网模式偏好（AUTO、仅5G、仅4G、4G+5G 等档位下发与回读确认）
 *   - 一键解除全部锁定（POST /api/lock/unlock）
 */

function rowKv(label, val) {
	return E('div', { 'class': 'mp-kv-item' }, [
		E('span', { 'class': 'mp-kv-label' }, label),
		E('span', { 'class': 'mp-kv-val' }, val == null || val === '' ? '-' : String(val))
	]);
}

return view.extend({
	load: function () {
		fmt.ensureCss();
		store.start(api);
		var id = store.id() != null ? store.id() : undefined;
		return Promise.all([
			store.fetch(api),
			api.get('/api/lock/status', { modem_id: id }).catch(function () { return {}; }),
			api.get('/api/mode', { modem_id: id }).catch(function () { return {}; })
		]);
	},

	render: function (data) {
		var self = this;
		var snap = data[0] || {};
		var lockSt = data[1] || (snap.blocks && snap.blocks.lock) || {};
		var modeSt = data[2] || {};
		var id = snap.modem_id != null ? snap.modem_id : 0;

		var curTab = 'status';

		/* 根节点容器 */
		var root = E('div', { 'class': 'mp-wrap' }, [
			fmt.renderTabBar('lock'),

			/* 1. 顶部 Header */
			E('div', { 'class': 'mp-header' }, [
				E('div', { 'style': 'display:flex;align-items:center;gap:12px;flex-wrap:wrap' }, [
					E('div', { 'class': 'mp-title-badge' }, E('span', { 'innerHTML': icons.lock(true, 18) })),
					E('div', {}, [
						E('h2', { 'class': 'mp-title-text' }, '5G 终端 · 锁频段 / 锁基站'),
						E('div', { 'class': 'mp-subtitle-text' },
							'射频频段锁定 · 小区频点锁定 · 组网接入制式偏好')
					])
				]),
				E('div', { 'style': 'display:flex;align-items:center;gap:8px;flex-wrap:wrap' }, [
					E('button', {
						'class': 'mp-btn mp-btn-danger mp-btn-sm',
						'title': '解除所有频段与小区锁定，恢复全网自适应',
						'click': ui.createHandlerFn(self, function (btn) {
							if (!confirm('确定要解除全部锁定（频段恢复默认 policy，小区锁定置零）吗？')) return;
							btn.disabled = true;
							var orig = btn.innerHTML;
							btn.textContent = '解除中…';
							return api.post('/api/lock/unlock', { modem_id: id }).then(function () {
								ui.addNotification(null, E('p', {}, '全部锁定已解除'), 'info');
								store.refresh();
								window.location.reload();
							}).catch(function (e) {
								ui.addNotification(null, E('p', {}, '解除失败: ' + (e.message || e)), 'error');
							}).finally(function () { btn.disabled = false; btn.innerHTML = orig; });
						})
					}, [ E('span', { 'innerHTML': icons.lock(false, 13) }), ' 全部解除锁定' ]),
					E('button', {
						'class': 'mp-btn mp-btn-primary mp-btn-sm',
						'click': ui.createHandlerFn(self, function (btn) {
							btn.disabled = true;
							var orig = btn.innerHTML;
							btn.textContent = '读取中…';
							task.run(api, 'lock.status', { modem_id: id, refresh: true }).then(function (st) {
								ui.addNotification(null, E('p', {}, '锁网状态已刷新'), 'info');
								store.refresh();
								window.location.reload();
							}).catch(function (e) {
								ui.addNotification(null, E('p', {}, '读取失败: ' + (e.message || e)), 'error');
							}).finally(function () { btn.disabled = false; btn.innerHTML = orig; });
						})
					}, [ E('span', { 'innerHTML': icons.refresh(13) }), ' 刷新状态' ])
				])
			]),

			/* 2. 子页签导航 */
			E('div', { 'class': 'mp-subtabs' }, [
				E('button', { 'class': 'mp-subtab-btn active', 'data-tab': 'status', 'click': function (e) { self.switchTab('status', e.target); } }, [
					E('span', { 'innerHTML': icons.pulse(15) }), ' 锁网状态'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'band', 'click': function (e) { self.switchTab('band', e.target); } }, [
					E('span', { 'innerHTML': icons.antenna(15) }), ' 锁频段'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'cell', 'click': function (e) { self.switchTab('cell', e.target); } }, [
					E('span', { 'innerHTML': icons.tower(15) }), ' 锁小区 (基站)'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'mode', 'click': function (e) { self.switchTab('mode', e.target); } }, [
					E('span', { 'innerHTML': icons.settings(15) }), ' 组网模式偏好'
				])
			]),

			/* 3. 子页面 1：锁网状态概览 */
			E('div', { 'id': 'sub-status', 'class': 'mp-subpane' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ E('span', { 'innerHTML': icons.antenna(17) }), 'LTE 频段锁定状态' ]),
						E('span', { 'class': 'mp-badge ' + ((lockSt.band_lte && lockSt.band_lte.locked) ? 'mp-badge-warning' : 'mp-badge-success') },
							(lockSt.band_lte && lockSt.band_lte.locked) ? '已锁定' : '未限制')
					]),
					E('div', { 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr' }, [
						rowKv('当前锁定值', (lockSt.band_lte && lockSt.band_lte.bands && lockSt.band_lte.bands.length) ? lockSt.band_lte.bands.map(function(b){return 'B'+b;}).join(', ') : '未限制（全频段）'),
						rowKv('设备物理能力', (lockSt.band_lte && lockSt.band_lte.capable && lockSt.band_lte.capable.length) ? lockSt.band_lte.capable.map(function(b){return 'B'+b;}).join(', ') : '固件未上报单独能力表'),
						rowKv('出厂清单支持', (lockSt.band_lte && lockSt.band_lte.declared && lockSt.band_lte.declared.length) ? lockSt.band_lte.declared.map(function(b){return 'B'+b;}).join(', ') : '参考标准频段')
					])
				]),
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ E('span', { 'innerHTML': icons.antenna(17) }), '5G NR 频段锁定状态' ]),
						E('span', { 'class': 'mp-badge ' + ((lockSt.band_nr && lockSt.band_nr.locked) ? 'mp-badge-warning' : 'mp-badge-success') },
							(lockSt.band_nr && lockSt.band_nr.locked) ? '已锁定' : '未限制')
					]),
					E('div', { 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr' }, [
						rowKv('当前锁定值', (lockSt.band_nr && lockSt.band_nr.bands && lockSt.band_nr.bands.length) ? lockSt.band_nr.bands.map(function(b){return 'n'+b;}).join(', ') : '未限制（全频段）'),
						rowKv('设备物理能力', (lockSt.band_nr && lockSt.band_nr.capable && lockSt.band_nr.capable.length) ? lockSt.band_nr.capable.map(function(b){return 'n'+b;}).join(', ') : '固件未上报单独能力表'),
						rowKv('出厂清单支持', (lockSt.band_nr && lockSt.band_nr.declared && lockSt.band_nr.declared.length) ? lockSt.band_nr.declared.map(function(b){return 'n'+b;}).join(', ') : '参考标准频段')
					])
				]),
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ E('span', { 'innerHTML': icons.settings(17) }), '模组锁网方言' ])
					]),
					E('div', { 'class': 'mp-kv-grid' }, [
						rowKv('锁网方言标准', lockSt.dialect_name || lockSt.dialect || '通用 3GPP'),
						rowKv('当前组网偏好', lockSt.mode_pref || modeSt.value || 'AUTO'),
						rowKv('方言特性说明', lockSt.dialect_note || '该平台锁频需离线后下发并自动切回正常模式')
					])
				])
			]),

			/* 4. 子页面 2：锁频段操作 */
			E('div', { 'id': 'sub-band', 'class': 'mp-subpane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ E('span', { 'innerHTML': icons.antenna(17) }), '频段选择与下发' ]),
						E('div', { 'class': 'mp-card-actions' }, [
							E('button', {
								'class': 'mp-btn mp-btn-primary mp-btn-sm',
								'click': ui.createHandlerFn(self, function () { return self.applyBands(id); })
							}, '下发勾选频段'),
							E('button', {
								'class': 'mp-btn mp-btn-ghost mp-btn-sm',
								'click': ui.createHandlerFn(self, function () { return self.unlockAll(id); })
							}, '解除全部')
						])
					]),
					E('div', { 'id': 'band-groups-container' }, [])
				])
			]),

			/* 5. 子页面 3：锁小区操作 */
			E('div', { 'id': 'sub-cell', 'class': 'mp-subpane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ E('span', { 'innerHTML': icons.tower(17) }), '锁小区 / 锁频点' ])
					]),
					E('div', { 'style': 'max-width:560px;display:flex;flex-direction:column;gap:14px' }, [
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:6px' }, '锁定类型:'),
							E('select', { 'id': 'cell-lock-type', 'class': 'mp-select', 'style': 'width:100%' }, [
								E('option', { 'value': '1' }, '锁定频点 + 物理小区 (EARFCN + PCI)'),
								E('option', { 'value': '2' }, '仅锁定物理频点 (仅 EARFCN)'),
								E('option', { 'value': '0' }, '解除小区锁定 (Lock Type = 0)')
							])
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:6px' }, '接入网络技术域 (RAT):'),
							E('select', { 'id': 'cell-lock-tech', 'class': 'mp-select', 'style': 'width:100%' }, [
								E('option', { 'value': 'LTE' }, 'LTE (4G)'),
								E('option', { 'value': 'NR' }, '5G NR')
							])
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:6px' }, '物理频点 (EARFCN / ARFCN):'),
							E('input', {
								'id': 'cell-lock-earfcn',
								'class': 'mp-input',
								'style': 'width:100%',
								'type': 'number',
								'placeholder': '例如 1300 / 1825 / 627264'
							})
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:6px' }, '物理小区标识 (PCI 0~1007):'),
							E('input', {
								'id': 'cell-lock-pcid',
								'class': 'mp-input',
								'style': 'width:100%',
								'type': 'number',
								'placeholder': '例如 119'
							})
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:6px' }, '5G 子载波间隔 (SCS 15/30/60/120，仅 5G NR 需指定):'),
							E('input', {
								'id': 'cell-lock-scs',
								'class': 'mp-input',
								'style': 'width:100%',
								'type': 'number',
								'placeholder': '30'
							})
						]),
						E('div', { 'style': 'padding-top:10px' }, [
							E('button', {
								'class': 'mp-btn mp-btn-primary',
								'click': ui.createHandlerFn(self, function (btn) {
									var ltype = parseInt(document.getElementById('cell-lock-type').value, 10);
									var tech = document.getElementById('cell-lock-tech').value;
									var earfcn = parseInt(document.getElementById('cell-lock-earfcn').value, 10);
									var pcid = parseInt(document.getElementById('cell-lock-pcid').value, 10);
									var scs = parseInt(document.getElementById('cell-lock-scs').value, 10);

									if (ltype !== 0 && isNaN(earfcn)) {
										alert('请输入有效的频点 (EARFCN)');
										return;
									}
									if (ltype === 1 && isNaN(pcid)) {
										alert('锁频点+PCI 必须输入 PCI');
										return;
									}

									btn.disabled = true;
									btn.textContent = '下发中…';
									api.post('/api/lock/cell', {
										modem_id: id,
										tech: tech,
										lock_type: ltype,
										earfcn: isNaN(earfcn) ? 0 : earfcn,
										pcid: isNaN(pcid) ? 0 : pcid,
										scs: isNaN(scs) ? 30 : scs
									}).then(function (r) {
										ui.addNotification(null, E('p', {}, '锁小区指令已执行'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '执行失败: ' + (e.message || e)), 'error');
										btn.disabled = false;
										btn.textContent = '执行锁小区';
									});
								})
							}, '执行锁小区')
						]),
						E('p', { 'style': 'font-size:12px;color:var(--mp-neutral-500);line-height:1.6;margin:0' },
							'注意：部分平台（如广和通 FM350 系列）未公开锁小区寄存器参数，该平台支持一键解锁（Lock Type = 0），下发锁定前请确认设备支持。')
					])
				])
			]),

			/* 6. 子页面 4：组网模式偏好 */
			E('div', { 'id': 'sub-mode', 'class': 'mp-subpane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ E('span', { 'innerHTML': icons.settings(17) }), '组网模式偏好 (Network Mode Preference)' ])
					]),
					E('div', { 'style': 'max-width:540px;display:flex;flex-direction:column;gap:14px' }, [
						E('p', { 'style': 'font-size:13px;color:var(--mp-neutral-700);margin:0' },
							'设置模组优先注册的无线电接入技术。下发后模组会自动与基站重新协商。'),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:6px' }, '选择目标模式:'),
							E('select', { 'id': 'sel-pref-mode', 'class': 'mp-select', 'style': 'width:100%' }, [
								E('option', { 'value': 'AUTO', 'selected': (modeSt.value === 'AUTO') }, 'AUTO (自动网络搜索，默认)'),
								E('option', { 'value': 'NR5G', 'selected': (modeSt.value === 'NR5G') }, '仅 5G NR (SA / NSA)'),
								E('option', { 'value': 'LTE', 'selected': (modeSt.value === 'LTE') }, '仅 4G LTE'),
								E('option', { 'value': 'LTE:NR5G', 'selected': (modeSt.value === 'LTE:NR5G' || modeSt.value === 'NR5G:LTE') }, '4G LTE + 5G NR 优先'),
								E('option', { 'value': 'WCDMA', 'selected': (modeSt.value === 'WCDMA') }, '仅 3G WCDMA'),
								E('option', { 'value': 'WCDMA:LTE', 'selected': (modeSt.value === 'WCDMA:LTE') }, '3G + 4G')
							])
						]),
						E('div', {}, [
							E('button', {
								'class': 'mp-btn mp-btn-primary',
								'click': ui.createHandlerFn(self, function (btn) {
									var targetVal = document.getElementById('sel-pref-mode').value;
									btn.disabled = true;
									btn.textContent = '写入中…';
									api.post('/api/mode/set', { modem_id: id, value: targetVal }).then(function () {
										ui.addNotification(null, E('p', {}, '组网模式偏好已保存为 ' + targetVal), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '切换失败: ' + (e.message || e)), 'error');
										btn.disabled = false;
										btn.textContent = '应用偏好设置';
									});
								})
							}, '应用偏好设置')
						])
					])
				])
			])
		]);

		this.drawBands(lockSt);
		return root;
	},

	switchTab: function (tabName, clickedBtn) {
		document.querySelectorAll('.mp-subtab-btn').forEach(function (b) { b.classList.remove('active'); });
		if (clickedBtn) clickedBtn.classList.add('active');

		document.querySelectorAll('.mp-subpane').forEach(function (p) { p.style.display = 'none'; });
		var target = document.getElementById('sub-' + tabName);
		if (target) target.style.display = 'block';
	},

	drawBands: function (st) {
		var container = document.getElementById('band-groups-container');
		if (!container) return;
		container.innerHTML = '';

		var groups = st.band_groups || [];
		if (!groups.length) {
			container.appendChild(E('p', { 'style': 'color:var(--mp-neutral-500);padding:14px 0' },
				'未获取到频段分组信息，请点击右上角「刷新状态」。'));
			return;
		}

		this._groups = {};
		var currents = {
			lte: (st.band_lte && st.band_lte.bands) || [],
			nr: (st.band_nr && st.band_nr.bands) || [],
			nr_nsa: (st.band_nr_nsa && st.band_nr_nsa.bands) || [],
			nr_sa: (st.band_nr && st.band_nr.bands) || []
		};

		var self = this;
		groups.forEach(function (g) {
			self._groups[g.key] = g;
			var wrap = E('div', { 'class': 'mp-chip-wrap', 'id': 'chipset-' + g.key });
			var curList = currents[g.key] || [];

			(g.bands || []).forEach(function (b) {
				var isLocked = curList.indexOf(b) >= 0;
				var chip = E('div', {
					'class': 'mp-chip ' + (isLocked ? 'active' : ''),
					'data-band': String(b)
				}, [
					(g.key === 'lte' ? 'B' : (g.key.indexOf('nr') >= 0 ? 'n' : '')) + b
				]);
				chip.addEventListener('click', function () {
					chip.classList.toggle('active');
				});
				wrap.appendChild(chip);
			});

			var sec = E('div', { 'style': 'margin-bottom:16px;padding-bottom:14px;border-bottom:1px solid var(--mp-border-glass-subtle)' }, [
				E('div', { 'style': 'display:flex;align-items:center;justify-content:space-between;margin-bottom:8px' }, [
					E('div', { 'style': 'font-weight:600;font-size:13.5px' }, [
						g.title || g.key.toUpperCase(),
						g.lockable ? '' : ' (仅展示)'
					]),
					g.lockable ? E('div', { 'style': 'display:flex;gap:6px' }, [
						E('button', {
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'click': function () {
								wrap.querySelectorAll('.mp-chip').forEach(function (c) { c.classList.add('active'); });
							}
						}, '全选'),
						E('button', {
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'click': function () {
								wrap.querySelectorAll('.mp-chip').forEach(function (c) { c.classList.remove('active'); });
							}
						}, '清空')
					]) : ''
				]),
				wrap,
				g.note ? E('p', { 'style': 'font-size:12px;color:var(--mp-neutral-500);margin:4px 0 0' }, g.note) : ''
			]);

			container.appendChild(sec);
		});
	},

	applyBands: function (id) {
		var groups = this._groups || {};
		var jobs = [];

		for (var key in groups) {
			var g = groups[key];
			if (!g.lockable) continue;
			var wrap = document.getElementById('chipset-' + key);
			if (!wrap) continue;

			var picked = [];
			wrap.querySelectorAll('.mp-chip.active').forEach(function (c) {
				picked.push(parseInt(c.dataset.band, 10));
			});
			jobs.push({ tech: g.tech || (key.indexOf('nr') >= 0 ? 'NR' : 'LTE'), bands: picked });
		}

		if (!jobs.length) {
			alert('没有可下发的分组');
			return;
		}

		return Promise.all(jobs.map(function (j) {
			return api.post('/api/lock/band', {
				modem_id: id,
				tech: j.tech,
				bands: j.bands
			});
		})).then(function () {
			ui.addNotification(null, E('p', {}, '频段锁定设置已下发并生效'), 'info');
			store.refresh();
			window.location.reload();
		}).catch(function (e) {
			ui.addNotification(null, E('p', {}, '写入失败: ' + (e.message || e)), 'error');
		});
	},

	unlockAll: function (id) {
		return api.post('/api/lock/unlock', { modem_id: id }).then(function () {
			ui.addNotification(null, E('p', {}, '已解除所有制式频段与小区锁定'), 'info');
			store.refresh();
			window.location.reload();
		}).catch(function (e) {
			ui.addNotification(null, E('p', {}, '解除失败: ' + (e.message || e)), 'error');
		});
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
