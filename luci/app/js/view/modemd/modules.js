'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.fmt as fmt';
'require modemd.icons as icons';

/* 5G 终端 · 支持模块：型号清单 + 逐型号 AT 能力计划与实测指令面。
 * 数据源：/api/modules 离线清单与指令计划，支持在网模组实测对比。
 */

function bandText(b, note) {
	if (!b) return note || '-';
	var seg = [];
	var part = function (label, arr, prefix) {
		if (arr && arr.length)
			seg.push(label + ' ' + arr.map(function (n) { return prefix + n; }).join(' ') + '（' + arr.length + '）');
	};
	part('LTE', b.lte, 'B');
	part('NR-SA', b.nr_sa, 'n');
	part('NR-NSA', b.nr_nsa, 'n');
	part('WCDMA', b.wcdma, 'B');
	if (!seg.length) return note || '（频段表里该型号未声明频段）';
	return seg.join(' · ') + (note ? ' —— ' + note : '');
}

return view.extend({
	load: function () {
		fmt.ensureCss();
		return Promise.all([
			api.get('/api/modules').catch(function (e) { return { error: e.message || String(e) }; }),
			store.fetch(api).catch(function () { return null; })
		]);
	},

	render: function (res) {
		var self = this;
		var cat = res[0] || {};
		var snap = res[1] || {};
		var models = cat.models || [];
		var vendors = cat.vendors || [];
		var curModemId = store.id() != null ? store.id() : (snap && snap.modem_id != null ? snap.modem_id : 0);
		var curModem = null;
		if (snap && snap.modems) {
			for (var i = 0; i < snap.modems.length; i++) {
				if (snap.modems[i].id === curModemId) { curModem = snap.modems[i]; break; }
			}
		}

		var selectedModel = '';

		var metaEl = E('div', { 'class': 'mp-desc', 'style': 'margin-bottom:12px;font-size:13px;line-height:1.6;' });
		function updateMeta() {
			var curDesc = curModem ? ('当前在网模组：' + (curModem.model || ('#' + curModemId))) : '当前无在网模组';
			metaEl.innerHTML = '共 <b>' + (cat.count || models.length) + '</b> 个型号 / <b>' +
				(cat.vendor_count || vendors.length || '-') + '</b> 家厂商 · 支持 USB <b>' +
				(cat.usb_count || '-') + '</b> 个 · 支持 PCIe <b>' + (cat.pcie_count || '-') + '</b> 个 · ' + curDesc;
		}
		updateMeta();

		var searchInput = E('input', {
			'type': 'text',
			'class': 'cbi-input-text',
			'placeholder': '搜型号 / 厂商 / 平台，如 RM520N、Fibocom、Unisoc...',
			'style': 'flex:2 1 200px;margin-right:8px;'
		});

		var vendorSelect = E('select', { 'class': 'cbi-input-select', 'style': 'flex:1 1 140px;margin-right:8px;' }, [
			E('option', { 'value': '' }, '全部厂商')
		]);
		vendors.forEach(function (v) {
			vendorSelect.appendChild(E('option', { 'value': v.vendor }, v.name + ' (' + v.count + ')'));
		});

		var kindSelect = E('select', { 'class': 'cbi-input-select', 'style': 'flex:1 1 140px;' }, [
			E('option', { 'value': '' }, '全部总线形态'),
			E('option', { 'value': 'pcie' }, '只看支持 PCIe 的'),
			E('option', { 'value': 'usb' }, '只看支持 USB 的')
		]);

		var listContainer = E('div', {
			'style': 'max-height:360px;overflow-y:auto;border:1px solid var(--mp-glass-border);border-radius:var(--mp-radius-sm);margin-top:12px;background:rgba(255,255,255,0.4);'
		});

		var planContainer = E('div', { 'class': 'mp-card', 'style': 'margin-top:16px;display:none;' });

		function filterModels() {
			var kw = (searchInput.value || '').trim().toLowerCase();
			var v = vendorSelect.value || '';
			var k = kindSelect.value || '';
			return models.filter(function (m) {
				if (v && m.vendor !== v) return false;
				if (k === 'pcie' && !(m.pcie_modes || []).length) return false;
				if (k === 'usb' && !(m.usb_modes || []).length) return false;
				if (!kw) return true;
				var str = ((m.model || '') + ' ' + (m.vendor || '') + ' ' + (m.vendor_name || '') + ' ' + (m.platform || '')).toLowerCase();
				return str.indexOf(kw) >= 0;
			});
		}

		function renderList() {
			listContainer.innerHTML = '';
			var rows = filterModels();
			if (!rows.length) {
				listContainer.appendChild(E('div', { 'style': 'padding:16px;text-align:center;color:var(--mp-text-muted);' }, '没有匹配的模组型号'));
				return;
			}

			var tbl = E('table', { 'class': 'mp-table', 'style': 'margin:0;width:100%;' }, [
				E('thead', {}, E('tr', {}, [
					E('th', { 'style': 'width:28%;' }, '模组型号'),
					E('th', { 'style': 'width:18%;' }, '厂商'),
					E('th', { 'style': 'width:20%;' }, '平台基带'),
					E('th', { 'style': 'width:17%;' }, 'USB 协议'),
					E('th', { 'style': 'width:17%;' }, 'PCIe 形态')
				]))
			]);

			var tbody = E('tbody', {});
			rows.forEach(function (m) {
				var isSelected = m.model === selectedModel;
				var tr = E('tr', {
					'style': 'cursor:pointer;transition:background 0.2s;' + (isSelected ? 'background:rgba(2,132,199,0.12);font-weight:600;' : ''),
					'click': function () {
						selectedModel = m.model;
						renderList();
						self.loadPlan(m.model, false, planContainer);
					}
				}, [
					E('td', { 'style': 'color:var(--mp-primary);font-family:monospace;font-size:13px;' }, (m.model || '').toUpperCase()),
					E('td', {}, E('span', { 'class': 'mp-pill' }, m.vendor_name || m.vendor)),
					E('td', { 'style': 'color:var(--mp-text-sub);' }, m.platform_name || m.platform || '-'),
					E('td', { 'style': 'color:var(--mp-text-sub);' }, m.usb_text || ((m.usb_modes || []).join('/') || '—')),
					E('td', { 'style': 'color:var(--mp-text-sub);' }, m.pcie_text || ((m.pcie_modes || []).join('/') || '—'))
				]);
				tbody.appendChild(tr);
			});
			tbl.appendChild(tbody);
			listContainer.appendChild(tbl);
		}

		searchInput.addEventListener('input', renderList);
		vendorSelect.addEventListener('change', renderList);
		kindSelect.addEventListener('change', renderList);

		var btnLive = E('button', {
			'class': 'mp-btn mp-btn-primary',
			'title': '用当前在网模组实测指令面'
		}, [icons.get('cpu', 15), ' 用当前模组实测']);

		btnLive.addEventListener('click', function () {
			selectedModel = '';
			renderList();
			self.loadPlan(null, true, planContainer, curModemId);
		});

		var btnReload = E('button', {
			'class': 'mp-btn mp-btn-default',
			'title': '重新读取支持清单'
		}, [icons.get('refresh', 15), ' 刷新清单']);

		btnReload.addEventListener('click', function () {
			btnReload.disabled = true;
			api.get('/api/modules').then(function (newCat) {
				btnReload.disabled = false;
				cat = newCat || {};
				models = cat.models || [];
				vendors = cat.vendors || [];
				updateMeta();
				renderList();
				ui.addNotification(null, E('p', {}, '支持模块清单已刷新'), 'info');
			}).catch(function (e) {
				btnReload.disabled = false;
				ui.addNotification(null, E('p', {}, '刷新清单失败: ' + (e.message || e)), 'error');
			});
		});

		renderList();

		return E('div', { 'class': 'mp-wrap' }, [
			fmt.renderTabBar('modules'),
			E('div', { 'class': 'mp-header' }, [
				E('div', { 'class': 'mp-title-row' }, [
					E('h2', { 'class': 'mp-title' }, '5G 终端 · 支持模块'),
					E('div', { 'class': 'mp-actions' }, [ btnLive, btnReload ])
				]),
				metaEl,
				E('div', { 'style': 'display:flex;flex-wrap:wrap;gap:8px;align-items:center;' }, [
					searchInput, vendorSelect, kindSelect
				])
			]),

			E('div', { 'class': 'mp-card', 'style': 'margin-top:16px;' }, [
				E('div', { 'class': 'mp-card-title', 'style': 'display:flex;align-items:center;gap:8px;' }, [
					icons.get('modem', 18), ' 模块支持目录 (点击任意型号查看 AT 指令面)'
				]),
				listContainer
			]),

			planContainer,

			E('div', { 'class': 'mp-desc', 'style': 'margin-top:16px;padding:12px;background:rgba(255,255,255,0.4);border-radius:var(--mp-radius-sm);font-size:12px;line-height:1.7;' }, [
				E('p', { 'style': 'margin:0 0 6px 0;' }, [
					E('strong', {}, '说明：'),
					'清单同步自 Modem-Pro 核心支持库。',
					E('strong', {}, 'USB 模式'), ' 表示该型号固件支持的数据协议（qmi / gobinet / ecm / mbim / rndis / ncm / rmnet / auto）；',
					E('strong', {}, 'PCIe 模式'), ' 表示支持的 PCIe MHI / WWAN 形态。'
				]),
				E('p', { 'style': 'margin:0;' },
					'点击型号可展开其完整的 AT 指令面、候选附加指令、解析规范和应答格式。在网模组可通过右上角直接调取实测指令面。'
				)
			])
		]);
	},

	loadPlan: function (model, isLive, container, modemId) {
		var self = this;
		container.style.display = 'block';
		container.innerHTML = '';
		container.appendChild(E('div', { 'style': 'padding:24px;text-align:center;color:var(--mp-text-muted);' }, [
			icons.get('refresh', 20), ' 正在读取 ' + (isLive ? '在网模组' : (model || '')) + ' 的 AT 指令面...'
		]));
		container.scrollIntoView({ behavior: 'smooth', block: 'nearest' });

		var req = isLive ? api.get('/api/modules', { modem_id: modemId != null ? modemId : 0 }) : api.get('/api/modules', { model: model });
		req.then(function (res) {
			var p = (res && res.plan) || {};
			self.renderPlan(p, container);
		}).catch(function (e) {
			container.innerHTML = '';
			container.appendChild(E('div', { 'style': 'padding:20px;color:var(--mp-danger);' }, [
				icons.get('warn', 18), ' 读取指令面失败: ' + (e.message || e)
			]));
		});
	},

	renderPlan: function (p, container) {
		container.innerHTML = '';
		if (!p) {
			container.appendChild(E('div', { 'style': 'padding:20px;color:var(--mp-text-muted);' }, '未获取到指令面数据'));
			return;
		}

		var titleRow = E('div', { 'class': 'mp-card-title', 'style': 'display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;' }, [
			E('div', { 'style': 'display:flex;align-items:center;gap:8px;' }, [
				icons.get('cpu', 18),
				E('span', { 'style': 'font-size:16px;font-weight:700;' }, 'AT 指令面：' + (p.model || '-').toUpperCase()),
				p.live ? E('span', { 'class': 'mp-pill mp-pill-success' }, '在网模组实测') : null,
				p.in_catalog === false ? E('span', { 'class': 'mp-pill mp-pill-warn' }, '不在目录库中') : null
			])
		]);

		var grid = E('div', { 'class': 'mp-grid', 'style': 'grid-template-columns:repeat(auto-fit, minmax(220px, 1fr));gap:12px;margin-bottom:16px;' });

		function addBox(k, v, isWide) {
			var box = E('div', {
				'class': 'mp-metric-box',
				'style': (isWide ? 'grid-column:1 / -1;' : '') + 'padding:10px 14px;'
			}, [
				E('div', { 'class': 'mp-metric-label', 'style': 'font-size:11px;' }, k),
				E('div', { 'class': 'mp-metric-value', 'style': 'font-size:13px;word-break:break-all;' }, v == null || v === '' ? '-' : String(v))
			]);
			grid.appendChild(box);
		}

		addBox('厂商', (p.vendor_name || p.vendor || '-') + (p.in_catalog ? '' : '（未入库）'));
		addBox('平台基带', p.platform_name || p.platform || '未识别');
		addBox('平台变体', p.platform_variant ? (p.platform_variant + (p.platform_variant_note ? ' — ' + p.platform_variant_note : '')) : '未识别');
		addBox('指令实现', p.cmd_impl || '-');
		addBox('USB 模式', (p.usb_modes && p.usb_modes.length) ? p.usb_modes.join(', ') : '✘');
		addBox('PCIe 模式', (p.pcie_modes && p.pcie_modes.length) ? p.pcie_modes.join(', ') : '✘');
		addBox('支持频段', bandText(p.bands, p.bands_note), true);
		addBox('锁网方言', p.lock_dialect || '无');
		addBox('USB 模式方言', p.usb_dialect || '无');
		addBox('组网模式方言', p.mode_dialect || '无');
		addBox('IMEI 方言', p.imei_dialect || '-');

		var notesDiv = E('div', { 'style': 'margin-bottom:16px;display:flex;flex-direction:column;gap:8px;' });
		if (p.cmd_impl_note) {
			notesDiv.appendChild(E('div', { 'class': 'mp-alert mp-alert-info', 'style': 'padding:8px 12px;font-size:12px;' }, [
				E('strong', {}, '实现说明：'), p.cmd_impl_note
			]));
		}
		if (p.quirk) {
			notesDiv.appendChild(E('div', { 'class': 'mp-alert mp-alert-warning', 'style': 'padding:8px 12px;font-size:12px;' }, [
				E('strong', {}, '机型特例：'), p.quirk
			]));
		}
		if (p.resolved && Object.keys(p.resolved).length) {
			var rItems = [];
			for (var rk in p.resolved) {
				rItems.push(rk + ' → ' + p.resolved[rk]);
			}
			notesDiv.appendChild(E('div', { 'class': 'mp-alert mp-alert-success', 'style': 'padding:8px 12px;font-size:12px;' }, [
				E('strong', {}, '运行期实测选中：'), rItems.join('　|　')
			]));
		}
		if (p.gaps && p.gaps.length) {
			var gapItems = [];
			for (var gi = 0; gi < p.gaps.length; gi++) {
				var gNote = (p.gap_notes && p.gap_notes[gi]) ? (' (' + p.gap_notes[gi] + ')') : '';
				gapItems.push(p.gaps[gi] + gNote);
			}
			notesDiv.appendChild(E('div', { 'class': 'mp-alert mp-alert-danger', 'style': 'padding:8px 12px;font-size:12px;' }, [
				E('strong', {}, '本型号未覆盖能力：'), gapItems.join('、')
			]));
		}

		var cmdList = E('div', { 'style': 'display:flex;flex-direction:column;gap:8px;' });
		(p.commands || []).forEach(function (c) {
			var onBadge = c.on
				? E('span', { 'class': 'mp-pill mp-pill-success' }, '已接入')
				: E('span', { 'class': 'mp-pill', 'style': 'background:rgba(148,163,184,0.18);color:var(--mp-text-sub);' }, '未接入');

			var srcBadge = c.source ? E('span', { 'class': 'mp-pill', 'style': 'margin-left:6px;' }, c.source) : null;
			var cmdCode = c.cmd ? E('code', { 'style': 'margin-left:8px;color:var(--mp-primary);font-weight:bold;' }, c.cmd) : null;

			var candsEl = null;
			if (c.candidates && c.candidates.length) {
				candsEl = E('div', { 'style': 'font-size:11px;color:var(--mp-text-sub);margin-top:4px;' }, [
					'候选/附加：',
					E('span', { 'style': 'font-family:monospace;' }, c.candidates.join('　'))
				]);
			}

			var parserEl = E('div', { 'style': 'font-size:11px;color:var(--mp-text-sub);margin-top:2px;' }, [
				'解析：' + (c.parser || '-'),
				c.format ? (' · 格式：' + c.format) : '',
				c.note ? (' · 说明：' + c.note) : ''
			]);

			var item = E('div', {
				'style': 'padding:10px 14px;background:rgba(255,255,255,0.6);border:1px solid var(--mp-glass-border);border-radius:var(--mp-radius-sm);'
			}, [
				E('div', { 'style': 'display:flex;align-items:center;flex-wrap:wrap;' }, [
					E('strong', { 'style': 'font-size:13px;margin-right:8px;' }, c.label || c.key),
					onBadge,
					srcBadge,
					cmdCode
				]),
				candsEl,
				parserEl
			]);
			cmdList.appendChild(item);
		});

		container.appendChild(titleRow);
		container.appendChild(grid);
		container.appendChild(notesDiv);
		container.appendChild(E('h4', { 'style': 'font-size:14px;margin:16px 0 8px 0;' }, 'AT 命令集与解析实现列表'));
		container.appendChild(cmdList);
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
