'use strict';
'require view';
'require ui';
'require fs';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';
'require modemd.icons as icons';

/* 设备中心（现代化白色毛玻璃总览视图）
 *
 * 全面按内置 WebUI 对齐：
 *   - 顶部模组切换、运行态徽章、快捷开关（飞行模式、漫游、重启、校时、重探）
 *   - 信号强度卡片（5 档动态 SVG 柱、RSRP/SINR/RSRQ/RSSI 4 宫格、3GPP 评级）
 *   - 网络接入卡片（制式、双工、频段、EARFCN、CS/PS/5G 注册态、APN、v4/v6、原文）
 *   - 实时流量与速率卡片（上下行 bps 速率、会话累计流量、一键清零）
 *   - SIM 卡信息（卡在位状态、ICCID、IMSI、电话号码）
 *   - 模组硬件参数（型号、厂商、固件、IMEI、温度、电压、AT 端口）
 *   - 基站小区与拨号链路摘要
 */

function metricBox(title, val, unit, sub, iconItem, svgBarItem) {
	var head = E('div', { 'class': 'mp-metric-head' }, [
		E('span', {}, title)
	]);
	if (iconItem) {
		var iconEl = icons.toDOM ? icons.toDOM(iconItem) : (iconItem instanceof Node ? iconItem : document.createTextNode(''));
		head.appendChild(E('span', { 'class': 'mp-metric-icon-wrap', 'style': 'display:inline-flex;align-items:center;' }, [ iconEl ]));
	}

	var valWrap = E('div', { 'class': 'mp-metric-value-wrap' }, [
		E('span', { 'class': 'mp-metric-value' }, val == null || val === '' ? '-' : String(val)),
		unit ? E('span', { 'class': 'mp-metric-unit' }, unit) : ''
	]);

	var box = E('div', { 'class': 'mp-metric-box' }, [ head, valWrap ]);

	if (svgBarItem) {
		var barEl = icons.toDOM ? icons.toDOM(svgBarItem) : (svgBarItem instanceof Node ? svgBarItem : document.createTextNode(''));
		box.appendChild(E('div', { 'class': 'mp-metric-bar-wrap', 'style': 'margin:8px 0 6px 0;width:100%;' }, [ barEl ]));
	}

	if (sub) {
		var subEl = typeof sub === 'string' ? document.createTextNode(sub) : (sub instanceof Node ? sub : (icons.toDOM ? icons.toDOM(sub) : sub));
		box.appendChild(E('div', { 'class': 'mp-metric-sub' }, [ subEl ]));
	}

	return box;
}

function kvItem(label, val, extra) {
	return E('div', { 'class': 'mp-kv-item' }, [
		E('span', { 'class': 'mp-kv-label' }, label),
		E('span', { 'class': 'mp-kv-val' }, [
			val == null || val === '' ? '-' : String(val),
			extra ? E('span', { 'style': 'margin-left:6px;font-size:11.5px;color:var(--mp-neutral-500)' }, extra) : ''
		])
	]);
}

return view.extend({
	load: function () {
		fmt.ensureCss();
		return Promise.all([
			api.get('/api/version').catch(function () { return {}; }),
			store.start(api),
			api.get('/api/changes', { limit: 5 }).catch(function () { return {}; }),
			fs.read('/etc/modemd/config.toml').catch(function () { return ''; })
		]);
	},

	render: function (data) {
		var self = this;
		var ver = data[0] || {};
		var changes = data[2] || {};
		var cfg = data[3] || '';

		var m = /\[server\][\s\S]*?listen\s*=\s*["']?[^"'\n]*:(\d+)/.exec(cfg || '');
		var webuiPort = m ? m[1] : '9527';
		var webuiUrl = 'http://' + window.location.hostname + ':' + webuiPort + '/';

		var root = E('div', { 'class': 'mp-wrap' }, [
			fmt.renderTabBar('overview', webuiPort),

			/* 1. 顶部 Header 条 */
			E('div', { 'class': 'mp-header' }, [
				E('div', { 'mp-header-left': '', 'style': 'display:flex;align-items:center;gap:12px;flex-wrap:wrap' }, [
					E('div', { 'class': 'mp-title-badge' }, '5G'),
					E('div', {}, [
						E('h2', { 'class': 'mp-title-text' }, '5G 终端 · ' + fmt.T('Device Center')),
						E('div', { 'class': 'mp-subtitle-text' },
							'modemd ' + (ver.version || 'v1.0') + ' · ' + (ver.platform || 'linux') + ' · 异步化架构')
					]),
					E('span', { 'id': 'mp-modem-badge', 'class': 'mp-badge mp-badge-primary', 'style': 'margin-left:8px' }, '检测中…')
				]),
				E('div', { 'class': 'mp-header-right' }, [
					E('label', { 'style': 'font-size:12.5px;color:var(--mp-neutral-700);font-weight:500' }, fmt.T('Select modem') + ':'),
					E('select', {
						'id': 'mp-sel-modem',
						'class': 'mp-select',
						'style': 'min-width:220px',
						'change': ui.createHandlerFn(self, function (ev) {
							store.select(parseInt(ev.target.value, 10));
						})
					}, []),
					E('button', {
						'class': 'mp-btn mp-btn-ghost mp-btn-sm',
						'title': '重新探测系统串口与模组',
						'click': ui.createHandlerFn(self, function (btn) {
							btn.disabled = true;
							var orig = btn.innerHTML;
							btn.textContent = '重探中…';
							api.get('/api/modems', { discover: 1 }).then(function () {
								ui.addNotification(null, E('p', {}, '模组重探完成'), 'info');
								store.refresh();
							}).catch(function (e) {
								ui.addNotification(null, E('p', {}, '重探失败: ' + (e.message || e)), 'error');
							}).finally(function () {
								btn.disabled = false;
								btn.innerHTML = orig;
							});
						})
					}, [ icons.get('refresh', 14), ' 刷新模组' ])
				])
			]),

			/* 2. 快捷控制与状态工具条 */
			E('div', { 'class': 'mp-card', 'style': 'padding:12px 18px;margin-bottom:14px;display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:10px' }, [
				E('div', { 'style': 'display:flex;align-items:center;gap:16px;flex-wrap:wrap' }, [
					E('div', { 'style': 'display:flex;align-items:center;gap:6px' }, [
						icons.get('airplane', 16),
						E('label', { 'style': 'font-size:12.5px;font-weight:500' }, '飞行模式:'),
						E('button', {
							'id': 'btn-toggle-airplane',
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'click': ui.createHandlerFn(self, function (btn) {
								var snap = store.snap() || {};
								var net = (snap.blocks && snap.blocks.network) || {};
								var cur = !!net.airplane;
								btn.disabled = true;
								api.post('/api/airplane/set', { modem_id: store.id() != null ? store.id() : 0, enabled: !cur }).then(function () {
									ui.addNotification(null, E('p', {}, cur ? '已关闭飞行模式' : '已开启飞行模式'), 'info');
									store.refresh();
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								}).finally(function () { btn.disabled = false; });
							})
						}, '检测中…')
					]),
					E('div', { 'style': 'display:flex;align-items:center;gap:6px' }, [
						icons.get('roam', 16),
						E('label', { 'style': 'font-size:12.5px;font-weight:500' }, '数据漫游:'),
						E('button', {
							'id': 'btn-toggle-roam',
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'click': ui.createHandlerFn(self, function (btn) {
								var snap = store.snap() || {};
								var net = (snap.blocks && snap.blocks.network) || {};
								var cur = net.roam_allowed !== false;
								btn.disabled = true;
								api.post('/api/roam/set', { on: !cur }).then(function () {
									ui.addNotification(null, E('p', {}, cur ? '已禁止数据漫游' : '已允许数据漫游'), 'info');
									store.refresh();
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								}).finally(function () { btn.disabled = false; });
							})
						}, '检测中…')
					])
				]),
				E('div', { 'style': 'display:flex;align-items:center;gap:8px;flex-wrap:wrap' }, [
					E('button', {
						'class': 'mp-btn mp-btn-ghost mp-btn-sm',
						'title': '把模组时钟校准到主机 UTC 时间',
						'click': ui.createHandlerFn(self, function (btn) {
							btn.disabled = true;
							var orig = btn.innerHTML;
							btn.textContent = '校准中…';
							api.post('/api/modem/clock', { modem_id: store.id() != null ? store.id() : 0 }).then(function () {
								ui.addNotification(null, E('p', {}, '模组时钟校准成功'), 'info');
								store.refresh();
							}).catch(function (e) {
								ui.addNotification(null, E('p', {}, '校准失败: ' + (e.message || e)), 'error');
							}).finally(function () { btn.disabled = false; btn.innerHTML = orig; });
						})
					}, [ icons.get('clock', 14), ' 同步时钟' ]),
					E('button', {
						'class': 'mp-btn mp-btn-ghost mp-btn-sm',
						'style': 'color:#b91c1c;border-color:rgba(239,68,68,0.3)',
						'title': '重启 5G 模组（下发 AT+CFUN=1,1）',
						'click': ui.createHandlerFn(self, function (btn) {
							if (!confirm('确定要重启模组吗？\n重启期间模组 AT 口会短暂离线约 30 秒。')) return;
							btn.disabled = true;
							var orig = btn.innerHTML;
							btn.textContent = '重启中…';
							api.post('/api/modem/reset', { modem_id: store.id() != null ? store.id() : 0 }).then(function (r) {
								ui.addNotification(null, E('p', {}, (r && r.msg) || '重启指令已下发'), 'info');
								store.refresh();
							}).catch(function (e) {
								ui.addNotification(null, E('p', {}, '重启失败: ' + (e.message || e)), 'error');
							}).finally(function () { btn.disabled = false; btn.innerHTML = orig; });
						})
					}, [ icons.get('reboot', 14), ' 重启模组' ])
				])
			]),

			/* 3. 异步任务进行中提示条 */
			E('div', { 'id': 'mp-tasks-bar', 'style': 'margin-bottom:14px' }, []),

			/* 4. 信号质量大卡片 (Signal Quality) */
			E('div', { 'class': 'mp-card' }, [
				E('div', { 'class': 'mp-card-hd' }, [
					E('h3', { 'class': 'mp-card-title' }, [
						icons.get('tower', 18),
						'信号质量'
					]),
					E('div', { 'class': 'mp-card-actions' }, [
						E('span', { 'id': 'sig-age', 'style': 'font-size:12px;color:var(--mp-neutral-500);margin-right:4px' }, ''),
						E('button', {
							'id': 'btn-refresh-sig',
							'class': 'mp-btn mp-btn-primary mp-btn-sm',
							'click': ui.createHandlerFn(self, function (btn) {
								btn.disabled = true;
								var orig = btn.innerHTML;
								btn.textContent = '刷新中…';
								task.run(api, 'signal.refresh', { modem_id: store.id() != null ? store.id() : 0 }).then(function () {
									ui.addNotification(null, E('p', {}, '信号刷新完成'), 'info');
									store.refresh();
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '刷新失败: ' + (e.message || e)), 'error');
								}).finally(function () { btn.disabled = false; btn.innerHTML = orig; });
							})
						}, [ icons.get('refresh', 13), ' 刷新信号' ])
					])
				]),
				E('div', { 'id': 'sig-grid', 'class': 'mp-grid-4' }, [
					metricBox('运营商', '加载中…', '', ''),
					metricBox('信号强度 · RSRP', '-', 'dBm', ''),
					metricBox('信噪比 · SINR', '-', 'dB', ''),
					metricBox('信号质量 · RSRQ', '-', 'dB', '')
				]),
				E('div', { 'id': 'sig-bottom', 'style': 'display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:12px;padding-top:10px;border-top:1px solid var(--mp-border-glass-subtle);font-size:12.5px;color:var(--mp-neutral-700)' }, [
					E('span', {}, 'RSSI(CSQ): -'),
					E('span', {}, '误码率 (BER): -'),
					E('span', {}, '漫游状态: -')
				])
			]),

			/* 5. 网络接入与注册状态卡片 (Network Registration) */
			E('div', { 'class': 'mp-card' }, [
				E('div', { 'class': 'mp-card-hd' }, [
					E('h3', { 'class': 'mp-card-title' }, [
						icons.get('antenna', 18),
						'网络接入与注册'
					]),
					E('div', { 'class': 'mp-card-actions' }, [
						E('span', { 'id': 'net-age', 'style': 'font-size:12px;color:var(--mp-neutral-500);margin-right:4px' }, ''),
						E('button', {
							'id': 'btn-refresh-net',
							'class': 'mp-btn mp-btn-primary mp-btn-sm',
							'click': ui.createHandlerFn(self, function (btn) {
								btn.disabled = true;
								var orig = btn.innerHTML;
								btn.textContent = '刷新中…';
								task.run(api, 'network.refresh', { modem_id: store.id() != null ? store.id() : 0 }).then(function () {
									ui.addNotification(null, E('p', {}, '网络状态已刷新'), 'info');
									store.refresh();
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '刷新失败: ' + (e.message || e)), 'error');
								}).finally(function () { btn.disabled = false; btn.innerHTML = orig; });
							})
						}, [ icons.get('refresh', 13), ' 刷新网络' ])
					])
				]),
				E('div', { 'id': 'net-kv', 'class': 'mp-kv-grid' }, []),
				E('div', { 'id': 'net-raw-box', 'style': 'margin-top:12px' }, [])
			]),

			/* 6. 实时流量与速率卡片 (Real-time Traffic) */
			E('div', { 'class': 'mp-card' }, [
				E('div', { 'class': 'mp-card-hd' }, [
					E('h3', { 'class': 'mp-card-title' }, [
						icons.get('gauge', 18),
						'实时速率与流量'
					]),
					E('div', { 'class': 'mp-card-actions' }, [
						E('button', {
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'title': '清零模组累计流量计数器（AT+QGDCNT=0）',
							'click': ui.createHandlerFn(self, function (btn) {
								if (!confirm('确定要清零模组累计流量统计吗？')) return;
								btn.disabled = true;
								api.post('/api/traffic/reset', { modem_id: store.id() != null ? store.id() : 0 }).then(function () {
									ui.addNotification(null, E('p', {}, '流量计数器已清零'), 'info');
									store.refresh();
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '清零失败: ' + (e.message || e)), 'error');
								}).finally(function () { btn.disabled = false; });
							})
						}, '累计清零')
					])
				]),
				E('div', { 'id': 'traffic-grid', 'class': 'mp-grid-4' }, [
					metricBox('上行速率', '0', 'bps', '实时发送速率', icons.arrowUp(16)),
					metricBox('下行速率', '0', 'bps', '实时接收速率', icons.arrowDown(16)),
					metricBox('本次累计上传', '0 B', '', '已发数据量'),
					metricBox('本次累计下载', '0 B', '', '已收数据量')
				])
			]),

			/* 7. 双列布局：SIM 卡信息 + 模组硬件 */
			E('div', { 'style': 'display:grid;grid-template-columns:repeat(auto-fit, minmax(320px, 1fr));gap:16px;margin-bottom:18px' }, [
				/* SIM 卡 */
				E('div', { 'class': 'mp-card', 'style': 'margin-bottom:0' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [
							icons.get('sim', 18, 'ready'),
							'SIM 卡信息'
						]),
						E('span', { 'id': 'sim-status-badge', 'class': 'mp-badge mp-badge-success' }, '就绪')
					]),
					E('div', { 'id': 'sim-kv', 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr' }, [])
				]),
				/* 模组硬件 */
				E('div', { 'class': 'mp-card', 'style': 'margin-bottom:0' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [
							icons.get('chip', 18, 'running'),
							'模组硬件参数'
						]),
						E('span', { 'id': 'mod-model-badge', 'class': 'mp-badge mp-badge-primary' }, '-')
					]),
					E('div', { 'id': 'mod-kv', 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr' }, [])
				])
			]),

			/* 8. 双列布局：基站小区摘要 + 拨号链路 */
			E('div', { 'style': 'display:grid;grid-template-columns:repeat(auto-fit, minmax(320px, 1fr));gap:16px;margin-bottom:18px' }, [
				/* 基站摘要 */
				E('div', { 'class': 'mp-card', 'style': 'margin-bottom:0' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [
							icons.get('antenna', 18),
							'基站与载波聚合'
						]),
						E('a', { 'class': 'mp-btn mp-btn-ghost mp-btn-sm', 'href': '#tab=cells' }, '基站详情 →')
					]),
					E('div', { 'id': 'cells-summary-kv', 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr' }, [])
				]),
				/* 拨号链路 */
				E('div', { 'class': 'mp-card', 'style': 'margin-bottom:0' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [
							icons.get('dial', 18),
							'拨号链路状态'
						]),
						E('a', { 'class': 'mp-btn mp-btn-ghost mp-btn-sm', 'href': '#tab=dial' }, '拨号设置 →')
					]),
					E('div', { 'id': 'dial-summary-kv', 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr' }, [])
				])
			]),

			/* 9. 最近波动记录提示条 */
			E('div', { 'id': 'recent-change-box', 'class': 'mp-card', 'style': 'padding:12px 18px;display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:10px' }, [
				E('div', { 'style': 'display:flex;align-items:center;gap:8px;font-size:12.5px;color:var(--mp-neutral-700)' }, [
					icons.get('pulse', 18),
					E('span', { 'style': 'font-weight:600' }, '最近网络变化:'),
					E('span', { 'id': 'recent-change-text' }, (changes.entries && changes.entries[0]) ? (changes.entries[0].time + ' · ' + changes.entries[0].field + ' 从 ' + changes.entries[0].old + ' 变更为 ' + changes.entries[0].val) : '暂无波动记录')
				]),
				E('a', { 'class': 'mp-btn mp-btn-ghost mp-btn-sm', 'href': '#tab=tools' }, '查看全部记录')
			])
		]);

		store.on(function (s) { self.redraw(s); });
		return root;
	},

	redraw: function (s) {
		var snap = s.snapshot || {};
		var id = snap.modem_id != null ? snap.modem_id : 0;
		var modems = s.modems || [];
		var curModem = modems.filter(function (m) { return m.id === id; })[0] || (modems[0] || {});

		/* 模组下拉 */
		var sel = document.getElementById('mp-sel-modem');
		if (sel) {
			sel.innerHTML = '';
			if (!modems.length) {
				sel.appendChild(E('option', {}, '未检测到模组'));
				sel.disabled = true;
			} else {
				modems.forEach(function (m) {
					var opt = E('option', { 'value': String(m.id) },
						'#' + m.id + ' · ' + (m.vendor_name || m.vendor || '模组') + ' ' + (m.model || '') +
						' (' + (m.at_port || '-') + ')');
					sel.appendChild(opt);
				});
				sel.disabled = false;
				sel.value = String(id);
			}
		}

		/* 模组运行状态 Badge */
		var mBadge = document.getElementById('mp-modem-badge');
		if (mBadge) {
			if (!modems.length) {
				mBadge.className = 'mp-badge mp-badge-danger';
				mBadge.textContent = '无模组';
			} else if (curModem.running) {
				mBadge.className = 'mp-badge mp-badge-success';
				mBadge.textContent = '在线 · 运行中';
			} else {
				mBadge.className = 'mp-badge mp-badge-warning';
				mBadge.textContent = '未就绪';
			}
		}

		/* 快捷开关：飞行模式与漫游 */
		var B = snap.blocks || {};
		var net = B.network || {};
		var btnAir = document.getElementById('btn-toggle-airplane');
		if (btnAir) {
			btnAir.textContent = net.airplane ? '已开启（点击关闭）' : '已关闭（点击开启）';
			btnAir.className = 'mp-btn mp-btn-sm ' + (net.airplane ? 'mp-btn-danger' : 'mp-btn-ghost');
		}

		var btnRoam = document.getElementById('btn-toggle-roam');
		if (btnRoam) {
			var rAllowed = net.roam_allowed !== false;
			btnRoam.textContent = rAllowed ? '允许（点击禁止）' : '禁止（点击允许）';
			btnRoam.className = 'mp-btn mp-btn-sm ' + (rAllowed ? 'mp-btn-primary' : 'mp-btn-ghost');
		}

		/* 任务进度条 */
		var tasksDiv = document.getElementById('mp-tasks-bar');
		if (tasksDiv) {
			var tasks = snap.tasks || [];
			tasksDiv.innerHTML = '';
			tasks.forEach(function (t) {
				tasksDiv.appendChild(E('div', {
					'class': 'mp-card',
					'style': 'padding:10px 16px;margin-bottom:8px;background:rgba(14,165,233,0.08);border-color:rgba(14,165,233,0.3);display:flex;align-items:center;justify-content:space-between;gap:12px'
				}, [
					E('div', { 'style': 'display:flex;align-items:center;gap:10px;font-size:12.5px' }, [
						E('span', { 'class': 'mp-badge mp-badge-primary' }, t.state),
						E('span', { 'style': 'font-weight:600' }, t.label || t.kind),
						t.progress ? E('span', { 'style': 'color:var(--mp-neutral-500)' }, t.progress) : '',
						E('span', { 'style': 'color:var(--mp-neutral-500)' }, ((t.elapsed_ms || 0) / 1000).toFixed(0) + 's')
					]),
					E('button', {
						'class': 'mp-btn mp-btn-danger mp-btn-sm',
						'click': ui.createHandlerFn(this, function () {
							api.taskCancel(t.id).then(function () { store.refresh(); });
						})
					}, '取消')
				]));
			});
		}

		/* 信号卡片 */
		var sig = B.signal || {};
		var grade = fmt.signalGrade(sig.rssi, sig.nr_rsrp != null ? sig.nr_rsrp : sig.rsrp, sig.nr_sinr != null ? sig.nr_sinr : sig.sinr);

		var sigGrid = document.getElementById('sig-grid');
		if (sigGrid) {
			sigGrid.innerHTML = '';
			var opText = net.operator || curModem.vendor_name || '运营商';
			var techTag = net.tech_label || net.tech || '4G LTE';

			var rsrpVal = sig.nr_rsrp != null ? sig.nr_rsrp : (sig.rsrp != null ? sig.rsrp : null);
			var rsrpPct = rsrpVal != null ? Math.max(0, Math.min(100, Math.round((rsrpVal - (-140)) / (-60 - (-140)) * 100))) : 0;
			var rsrpColor = rsrpVal >= -85 ? '#10b981' : (rsrpVal >= -95 ? '#06b6d4' : (rsrpVal >= -105 ? '#0284c7' : (rsrpVal >= -115 ? '#f59e0b' : '#ef4444')));
			var rsrpBar = icons.signalBar(rsrpPct, rsrpColor, 8);

			var sinrVal = sig.nr_sinr != null ? sig.nr_sinr : (sig.sinr != null ? sig.sinr : null);
			var sinrPct = sinrVal != null ? Math.max(0, Math.min(100, Math.round((sinrVal - (-10)) / (30 - (-10)) * 100))) : 0;
			var sinrColor = sinrVal >= 20 ? '#10b981' : (sinrVal >= 13 ? '#06b6d4' : (sinrVal >= 7 ? '#0284c7' : (sinrVal >= 0 ? '#f59e0b' : '#ef4444')));
			var sinrBar = icons.signalBar(sinrPct, sinrColor, 8);

			var rsrqVal = sig.nr_rsrq != null ? sig.nr_rsrq : (sig.rsrq != null ? sig.rsrq : null);
			var rsrqPct = rsrqVal != null ? Math.max(0, Math.min(100, Math.round((rsrqVal - (-25)) / (-5 - (-25)) * 100))) : 0;
			var rsrqColor = rsrqVal >= -10 ? '#10b981' : (rsrqVal >= -15 ? '#0284c7' : (rsrqVal >= -20 ? '#f59e0b' : '#ef4444'));
			var rsrqBar = icons.signalBar(rsrqPct, rsrqColor, 8);

			var netBar = icons.signalBar(grade.bars * 20, grade.color, 8);

			sigGrid.appendChild(metricBox('当前网络', opText, '', E('span', { 'class': 'mp-badge mp-badge-primary' }, techTag), icons.get('antenna', 20), netBar));
			sigGrid.appendChild(metricBox('信号强度 · RSRP', rsrpVal != null ? rsrpVal : '-', 'dBm',
				E('span', { 'class': 'mp-badge mp-badge-' + (grade.level === 'great' || grade.level === 'good' ? 'success' : (grade.level === 'fair' ? 'warning' : 'danger')) }, grade.text),
				icons.signal(grade.bars, 26, rsrpColor), rsrpBar));
			sigGrid.appendChild(metricBox('信噪比 · SINR', sinrVal != null ? sinrVal : '-', 'dB',
				grade.text === '极好' || grade.text === '良好' ? '质量上乘' : '信噪一般',
				icons.signalTowerRipples(grade.bars, 22), sinrBar));
			sigGrid.appendChild(metricBox('信号质量 · RSRQ', rsrqVal != null ? rsrqVal : '-', 'dB',
				rsrqVal != null ? (rsrqVal >= -10 ? '优秀' : (rsrqVal >= -15 ? '良好' : '较差')) : '-',
				icons.pulse(22), rsrqBar));
		}

		var sigBottom = document.getElementById('sig-bottom');
		if (sigBottom) {
			sigBottom.innerHTML = '';
			var csqVal = (sig.rssi != null && sig.rssi !== 99) ? sig.rssi : null;
			var csqPct = csqVal != null ? Math.max(0, Math.min(100, Math.round(csqVal / 31 * 100))) : 0;
			var csqColor = csqVal >= 25 ? '#10b981' : (csqVal >= 18 ? '#0284c7' : (csqVal >= 12 ? '#f59e0b' : '#ef4444'));
			var csqIcon = icons.toDOM(icons.signal(fmt.bars(sig.rssi), 18, csqColor));

			sigBottom.appendChild(E('span', { 'style': 'display:inline-flex;align-items:center;gap:6px;' }, [
				csqIcon,
				document.createTextNode('RSSI(CSQ): ' + fmt.rssiText(sig.rssi) + ' (' + fmt.bars(sig.rssi) + '/5 格)')
			]));
			sigBottom.appendChild(E('span', {}, '误码率 (BER): ' + (sig.ber != null && sig.ber !== 99 ? sig.ber : '99 (未上报)')));
			sigBottom.appendChild(E('span', {}, '数据漫游: ' + (net.roaming ? '漫游中' : '本地网')));
		}

		var sigAge = document.getElementById('sig-age');
		if (sigAge) {
			var sAge = (snap.ages || {}).signal;
			sigAge.textContent = sAge != null ? ('采样于 ' + fmt.ago(sAge)) : '';
		}

		/* 网络接入卡片 */
		var netKv = document.getElementById('net-kv');
		if (netKv) {
			netKv.innerHTML = '';
			netKv.appendChild(kvItem('网络运营商', net.operator || '-', net.plmn ? 'PLMN: ' + net.plmn : ''));
			netKv.appendChild(kvItem('接入制式 / 双工', (net.tech_label || net.tech || '-') + (net.duplex ? ' (' + net.duplex + ')' : '')));
			netKv.appendChild(kvItem('工作频段', net.band_label || (net.band ? 'B' + net.band : '-'), net.band_text || ''));
			netKv.appendChild(kvItem('物理频点 (EARFCN)', net.earfcn || '-'));
			netKv.appendChild(kvItem('接入点 APN', net.apn || '-'));
			netKv.appendChild(kvItem('IPv4 本机地址', net.ipv4 || '-'));
			netKv.appendChild(kvItem('IPv6 本机地址', net.ipv6 || '-'));

			var regDesc = 'PS: ' + fmt.regText(net.ps_reg);
			if (net.cs_reg) regDesc += ' · CS: ' + fmt.regText(net.cs_reg);
			if (net.reg_5g && net.reg_5g.registered) regDesc += ' · 5GS: 已注册';
			netKv.appendChild(kvItem('网络注册状态', regDesc));
		}

		var netRawBox = document.getElementById('net-raw-box');
		if (netRawBox) {
			netRawBox.innerHTML = '';
			if (net.raw_net_info) {
				netRawBox.appendChild(E('div', { 'style': 'font-size:12px;color:var(--mp-neutral-500);margin-bottom:4px' }, '模组网络上报原文 (QNWINFO):'));
				netRawBox.appendChild(E('div', { 'class': 'mp-code-box' }, net.raw_net_info));
			}
		}

		var netAge = document.getElementById('net-age');
		if (netAge) {
			var nAge = (snap.ages || {}).network;
			netAge.textContent = nAge != null ? ('采样于 ' + fmt.ago(nAge)) : '';
		}

		/* 实时流量与速率 */
		var tr = net.traffic || {};
		var trafGrid = document.getElementById('traffic-grid');
		if (trafGrid) {
			trafGrid.innerHTML = '';
			trafGrid.appendChild(metricBox('实时上行', tr.up_text || fmt.bpsRate(tr.up_bps), '', '瞬时发送速率', icons.arrowUp(16)));
			trafGrid.appendChild(metricBox('实时下行', tr.down_text || fmt.bpsRate(tr.down_bps), '', '瞬时接收速率', icons.arrowDown(16)));
			trafGrid.appendChild(metricBox('累计已发送', fmt.bytes(tr.sent), '', '会话发送总量'));
			trafGrid.appendChild(metricBox('累计已接收', fmt.bytes(tr.recv), '', '会话接收总量'));
		}

		/* SIM 卡信息 */
		var simKv = document.getElementById('sim-kv');
		if (simKv) {
			simKv.innerHTML = '';
			simKv.appendChild(kvItem('SIM 卡在位状态', curModem.sim_state || (curModem.running ? '正常就绪' : '未检测到')));
			simKv.appendChild(kvItem('ICCID 卡号', curModem.iccid || '-'));
			simKv.appendChild(kvItem('IMSI 国际识别码', curModem.imsi || '-'));
			simKv.appendChild(kvItem('SIM 本机电话号', curModem.phone_number || '卡内未存'));
			simKv.appendChild(kvItem('SIM 归属运营商', curModem.sim_operator || net.operator || '-'));
		}

		/* 模组硬件 */
		var modKv = document.getElementById('mod-kv');
		if (modKv) {
			modKv.innerHTML = '';
			modKv.appendChild(kvItem('模组型号', curModem.model || '-'));
			modKv.appendChild(kvItem('设备厂商', curModem.vendor_name || curModem.vendor || '-'));
			modKv.appendChild(kvItem('IMEI 设备序列号', curModem.imei || '-'));
			modKv.appendChild(kvItem('当前 AT 命令端口', curModem.at_port || '-', curModem.at_port_manual ? '(已手动锁定)' : '(自动探测)'));
			modKv.appendChild(kvItem('芯片工作温度', curModem.temperature ? (curModem.temperature + ' ℃') : (curModem.temp_max ? (curModem.temp_max / 1000).toFixed(1) + ' ℃' : '-')));
			modKv.appendChild(kvItem('供电电压', curModem.voltage ? (curModem.voltage + ' mV') : '-'));
		}

		var mModelBadge = document.getElementById('mod-model-badge');
		if (mModelBadge) {
			mModelBadge.textContent = (curModem.model || 'MODEM').toUpperCase();
		}

		/* 基站小区与拨号摘要 */
		var cells = B.cells || {};
		var cellsKv = document.getElementById('cells-summary-kv');
		if (cellsKv) {
			cellsKv.innerHTML = '';
			cellsKv.appendChild(kvItem('服务小区数量', String(cells.serving_count || 0)));
			cellsKv.appendChild(kvItem('邻区基站数量', String(cells.neighbour_count || 0)));
			cellsKv.appendChild(kvItem('载波聚合 (CA) 状态', cells.ca_active ? '已激活' : '未激活 / 单载波'));
			if (cells.ca_note) cellsKv.appendChild(kvItem('CA 指令说明', cells.ca_note));
		}

		var dialSt = (snap.dial && snap.dial.state) || {};
		var dialKv = document.getElementById('dial-summary-kv');
		if (dialKv) {
			dialKv.innerHTML = '';
			dialKv.appendChild(kvItem('拨号连接状态', dialSt.state_text || dialSt.state || '未拨号'));
			dialKv.appendChild(kvItem('接口驱动类型', dialSt.type_name || dialSt.kind || '-'));
			dialKv.appendChild(kvItem('主机网卡接口', dialSt.host_iface || dialSt.iface || '-'));
			dialKv.appendChild(kvItem('已分配 IPv4', dialSt.ipv4 || '-'));
			dialKv.appendChild(kvItem('已连接时长', dialSt.elapsed_sec ? Math.round(dialSt.elapsed_sec / 60) + ' 分钟' : '-'));
		}
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
