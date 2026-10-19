'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';
'require modemd.icons as icons';

/* 调试工具与高级配置视图（白色毛玻璃全能工作台）
 *
 * 全面对齐 WebUI：
 *   1. AT 调试台（厂商常用指令预设、单行指令直发、应答回显、AT 命令口切换/探测）
 *   2. USSD 业务（USSD 码下发、异步 URC 回执查询）
 *   3. 主动搜网（LTE/NR 制式扫描、时长、可见运营商状态表、停止扫描）
 *   4. 流量配额（超限断网/关射频/仅告警、限额GB/MB、重置日、清零、解阻断）
 *   5. USB 网卡模式（当前模式、档位指令、安全阻断保护、下发切换）
 *   6. IMEI 管理（读取、Luhn校验、自动备份、改写写入、一键恢复原始出厂值）
 *   7. 定时器任务（计划任务增删改查、动作预检、启停开关、调试立即跑）
 *   8. 设备与热插拔（USB/PCIe 设备清单、自动补驱动、串口绑定、PCIe 软复位）
 *   9. 波动记录（信号/网络实时波动事件列表、一键清空）
 *  10. 运行日志（modemd 守护进程实时日志尾部拉取）
 */

function metricBox(label, val, unit, sub) {
	return E('div', { 'class': 'mp-metric-box' }, [
		E('div', { 'class': 'mp-metric-head' }, [
			E('span', { 'class': 'mp-metric-label' }, label)
		]),
		E('div', { 'class': 'mp-metric-value-wrap' }, [
			E('span', { 'class': 'mp-metric-value' }, val == null || val === '' ? '-' : String(val)),
			unit ? E('span', { 'class': 'mp-metric-unit' }, unit) : ''
		]),
		sub ? E('div', { 'class': 'mp-metric-sub' }, sub) : ''
	]);
}

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
			api.get('/api/at/presets', { modem_id: id }).catch(function () { return { presets: [] }; }),
			api.get('/api/quota', { modem_id: id }).catch(function () { return {}; }),
			api.get('/api/usbnet', { modem_id: id }).catch(function () { return {}; }),
			api.get('/api/imei', { modem_id: id }).catch(function () { return {}; }),
			api.get('/api/schedule').catch(function () { return { tasks: [] }; }),
			api.get('/api/usb').catch(function () { return {}; }),
			api.get('/api/pcie').catch(function () { return {}; }),
			api.get('/api/changes', { limit: 100 }).catch(function () { return { entries: [] }; }),
			api.get('/api/logs', { lines: 100 }).catch(function () { return { lines: [] }; })
		]);
	},

	/* 拉一次串口调度状态，更新徽章。
	   返回 false 表示「工具页已经不在了」，调用方的定时器据此自停。 */
	loadAtSched: function (id) {
		if (!document.getElementById('at-sched-badge')) return false;
		api.get('/api/at/sched', { modem_id: id }).then(function (st) {
			var b = document.getElementById('at-sched-badge');
			if (!b || !st) return;
			/* waiting == usize::MAX 时后端的意思是「调度器正卡在限流 sleep 里、
			   拿不到快照」，不是真有天文数字个等待者 —— 这里如实折成「忙」。 */
			var waiting = st.waiting;
			var busyNow = st.busy || waiting > 100000;
			var parts = [];
			if (busyNow) parts.push('串口中');
			if (waiting > 0 && waiting < 100000) parts.push('排队 ' + waiting);
			var thr = ((st.signal && st.signal.throttled) || 0)
				+ ((st.data && st.data.throttled) || 0)
				+ ((st.bulk && st.bulk.throttled) || 0)
				+ ((st.user && st.user.throttled) || 0);
			if (thr > 0) parts.push('限流 ' + thr);
			if (!parts.length) parts.push('空闲');

			b.textContent = parts.join(' · ');
			/* 只用语义色，不用红绿（那是涨跌语义）：空闲=中性、忙=提示、限流多=警告 */
			b.className = 'mp-badge ' + (thr > 20 ? 'mp-badge-warning' : (busyNow ? 'mp-badge-info' : ''));
			var lim = st.limits || {};
			b.title = '串口调度 · 最长单次等待 ' + (st.max_wait_ms || 0) + ' ms；'
				+ '服务次数 用户=' + ((st.user || {}).served || 0)
				+ ' 信号=' + ((st.signal || {}).served || 0)
				+ ' 数据=' + ((st.data || {}).served || 0)
				+ ' 冷门=' + ((st.bulk || {}).served || 0) + '；'
				+ '限流阈值 ' + (lim.per_window || '-') + ' 次/' + ((lim.window_ms || 0) / 1000) + ' 秒'
				+ '，最小间隔 ' + (lim.gap_ms || 0) + ' ms；'
				+ '优先级 用户手动 AT > 信号 > 拨号/IP/CA > 冷门查询';
		}).catch(function () { /* 拉不到不打扰用户：徽章保持上一帧 */ });
		return true;
	},

	render: function (data) {
		var self = this;
		var snap = data[0] || {};
		var presetsObj = data[1] || {};
		var presets = Array.isArray(presetsObj) ? presetsObj : (presetsObj.presets || []);
		var quota = data[2] || {};
		var usbnet = data[3] || {};
		var imei = data[4] || {};
		var schedule = data[5] || {};
		var usbDev = data[6] || {};
		var pcieDev = data[7] || {};
		var changes = data[8] || {};
		var logs = data[9] || {};
		var id = snap.modem_id != null ? snap.modem_id : 0;

		/* 串口调度徽章：6 秒刷一次；工具页被卸载后 loadAtSched 返回 false，定时器自停。
		   不用 store 的全局轮询，是因为这个状态只在 AT 调试台上有意义。 */
		var schedTimer = setInterval(function () {
			if (!self.loadAtSched(id)) clearInterval(schedTimer);
		}, 6000);
		self.loadAtSched(id);

		/* 新增定时任务弹窗 */
		function openScheduleModal() {
			var mask = E('div', { 'class': 'mp-modal-mask' }, [
				E('div', { 'class': 'mp-modal' }, [
					E('div', { 'class': 'mp-modal-hd' }, [
						E('h4', { 'class': 'mp-modal-title' }, '新建定时任务'),
						E('button', {
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'click': function () { document.body.removeChild(mask); }
						}, '✕')
					]),
					E('div', { 'style': 'display:flex;flex-direction:column;gap:12px' }, [
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '任务名称:'),
							E('input', { 'id': 'sched-name', 'class': 'mp-input', 'style': 'width:100%', 'placeholder': '例如：白天锁 n78' })
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '执行动作:'),
							E('select', { 'id': 'sched-action', 'class': 'mp-select', 'style': 'width:100%' }, [
								E('option', { 'value': 'band_lock' }, '锁定频段 (band_lock)'),
								E('option', { 'value': 'band_unlock' }, '解除频段锁定 (band_unlock)'),
								E('option', { 'value': 'unlock_all' }, '一键解除全部锁定 (unlock_all)'),
								E('option', { 'value': 'mode_pref' }, '设置组网偏好模式 (mode_pref)'),
								E('option', { 'value': 'airplane_on' }, '开启飞行模式 (airplane_on)'),
								E('option', { 'value': 'airplane_off' }, '关闭飞行模式 (airplane_off)')
							])
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '技术域 / 频段参数 (如需):'),
							E('div', { 'style': 'display:flex;gap:8px' }, [
								E('select', { 'id': 'sched-tech', 'class': 'mp-select', 'style': 'width:110px' }, [
									E('option', { 'value': 'NR' }, '5G NR'),
									E('option', { 'value': 'LTE' }, '4G LTE')
								]),
								E('input', { 'id': 'sched-bands', 'class': 'mp-input', 'style': 'flex:1', 'placeholder': '频段，如 78 或 3,8' })
							])
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '开始执行时间 (HH:MM):'),
							E('input', { 'id': 'sched-time', 'class': 'mp-input', 'style': 'width:100%', 'placeholder': '08:00' })
						])
					]),
					E('div', { 'class': 'mp-modal-ft' }, [
						E('button', {
							'class': 'mp-btn mp-btn-ghost',
							'click': function () { document.body.removeChild(mask); }
						}, '取消'),
						E('button', {
							'class': 'mp-btn mp-btn-primary',
							'click': function (b) {
								var name = (document.getElementById('sched-name').value || '').trim();
								var act = document.getElementById('sched-action').value;
								var tech = document.getElementById('sched-tech').value;
								var bands = (document.getElementById('sched-bands').value || '').trim();
								var stime = (document.getElementById('sched-time').value || '').trim();

								if (!stime) { alert('请输入执行时间'); return; }
								b.target.disabled = true;
								b.target.textContent = '保存中…';

								var payload = {
									modem_id: id,
									name: name,
									start_time: stime,
									start_actions: [{ kind: act, tech: tech, bands: bands }]
								};

								api.post('/api/schedule/set', payload).then(function () {
									ui.addNotification(null, E('p', {}, '定时任务已创建'), 'info');
									document.body.removeChild(mask);
									window.location.reload();
								}).catch(function (e) {
									alert('创建失败: ' + (e.message || e));
									b.target.disabled = false;
									b.target.textContent = '保存';
								});
							}
						}, '保存任务')
					])
				])
			]);
			document.body.appendChild(mask);
		}

		/* 根布局 */
		var root = E('div', { 'class': 'mp-wrap' }, [
			fmt.renderTabBar('tools'),

			/* 1. Header */
			E('div', { 'class': 'mp-header' }, [
				E('div', { 'style': 'display:flex;align-items:center;gap:12px;flex-wrap:wrap' }, [
					E('div', { 'class': 'mp-title-badge' }, icons.get('terminal', 18)),
					E('div', {}, [
						E('h2', { 'class': 'mp-title-text' }, '5G 终端 · 调试工具与高级配置'),
						E('div', { 'class': 'mp-subtitle-text' },
							'AT 调试台 · USSD · 搜网 · 流量配额 · USB/PCIe 硬件管理 · 定时器 · 波动与日志')
					])
				])
			]),

			/* 2. 10 宫格子页签导航 */
			E('div', { 'class': 'mp-subtabs' }, [
				E('button', { 'class': 'mp-subtab-btn active', 'data-tab': 'at', 'click': function (e) { self.switchTab('at', e.target); } }, [
					icons.get('terminal', 14), ' AT 调试台'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'ussd', 'click': function (e) { self.switchTab('ussd', e.target); } }, [
					icons.get('antenna', 14), ' USSD'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'netscan', 'click': function (e) { self.switchTab('netscan', e.target); } }, [
					icons.get('tower', 14), ' 主动搜网'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'quota', 'click': function (e) { self.switchTab('quota', e.target); } }, [
					icons.get('gauge', 14), ' 流量配额'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'usbnet', 'click': function (e) { self.switchTab('usbnet', e.target); } }, [
					icons.get('usb', 14), ' USB 网卡模式'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'imei', 'click': function (e) { self.switchTab('imei', e.target); } }, [
					icons.get('chip', 14, 'running'), ' IMEI 管理'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'schedule', 'click': function (e) { self.switchTab('schedule', e.target); } }, [
					icons.get('calendar', 14), ' 定时任务'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'devices', 'click': function (e) { self.switchTab('devices', e.target); } }, [
					icons.get('pcie', 14), ' 设备与热插拔'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'changes', 'click': function (e) { self.switchTab('changes', e.target); } }, [
					icons.get('pulse', 14), ' 波动记录'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'logs', 'click': function (e) { self.switchTab('logs', e.target); } }, [
					icons.get('logs', 14), ' 运行日志'
				])
			]),

			/* ---------------- Tab 1: AT 调试台 ---------------- */
			E('div', { 'id': 'tool-tab-at', 'class': 'mp-tool-pane' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('terminal', 17), 'AT 命令调试控制台' ]),
						E('div', { 'style': 'display:flex;align-items:center;gap:8px' }, [
							E('label', { 'style': 'font-size:12px;color:var(--mp-neutral-500)' }, 'AT 命令口:'),
							E('input', {
								'id': 'at-port-input',
								'class': 'mp-input',
								'style': 'width:150px',
								'placeholder': '/dev/ttyUSB4',
								'value': snap.modems && snap.modems[0] ? (snap.modems[0].at_port || '') : ''
							}),
							E('button', {
								'class': 'mp-btn mp-btn-ghost mp-btn-sm',
								'title': '切换到指定的命令口并记住，留空恢复自动探测',
								'click': ui.createHandlerFn(self, function (b) {
									var p = (document.getElementById('at-port-input').value || '').trim();
									b.disabled = true;
									api.post('/api/at/port', { modem_id: id, port: p }).then(function (r) {
										ui.addNotification(null, E('p', {}, (r && r.msg) || 'AT 端口已切换'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '切换失败: ' + (e.message || e)), 'error');
										b.disabled = false;
									});
								})
							}, '切换端口')
						])
					]),
					/* 常用指令预设选择 */
					E('div', { 'style': 'margin-bottom:12px;display:flex;align-items:center;gap:8px;flex-wrap:wrap' }, [
						E('span', { 'style': 'font-size:12.5px;color:var(--mp-neutral-700);font-weight:500' }, '常用指令预设:'),
						E('select', {
							'id': 'at-preset-sel',
							'class': 'mp-select',
							'style': 'min-width:280px',
							'change': function (ev) {
								if (ev.target.value) document.getElementById('at-cmd-in').value = ev.target.value;
							}
						}, [
							E('option', { 'value': '' }, '— 选择厂商与标准预设指令 —'),
							presets.map(function (p) {
								return E('option', { 'value': p.cmd || p }, (p.group ? '[' + p.group + '] ' : '') + (p.desc || p.label || p.cmd || p) + ' (' + (p.cmd || p) + ')');
							})
						])
					]),
					/* 指令输入栏 */
					E('div', { 'style': 'display:flex;gap:8px;margin-bottom:12px' }, [
						E('input', {
							'id': 'at-cmd-in',
							'class': 'mp-input',
							'style': 'flex:1;font-family:ui-monospace,monospace',
							'placeholder': '输入单条 AT 指令，例如 AT+CSQ / ATI / AT+QENG="servingcell"'
						}),
						E('button', {
							'id': 'btn-at-send',
							'class': 'mp-btn mp-btn-primary',
							'click': ui.createHandlerFn(self, function (btn) {
								var cmd = (document.getElementById('at-cmd-in').value || '').trim();
								if (!cmd) return;
								btn.disabled = true;
								var orig = btn.textContent;
								btn.textContent = '发送中…';
								var out = document.getElementById('at-console-out');

								task.run(api, 'at', { modem_id: id, cmd: cmd }, { timeout: 60000 })
									.then(function (res) {
										var lines = (res && res.resp) || [];
										out.textContent += '> ' + cmd + '\n' + (lines.join('\n') || '(无内容)') + '\n\n';
										out.scrollTop = out.scrollHeight;
									})
									.catch(function (e) {
										out.textContent += '> ' + cmd + '\n[执行失败] ' + (e.message || e) + '\n\n';
										out.scrollTop = out.scrollHeight;
									})
									.finally(function () {
										btn.disabled = false;
										btn.textContent = orig;
									});
							})
						}, '发送指令 (Enter)')
					]),
					/* 回显控制台 */
					E('pre', {
						'id': 'at-console-out',
						'class': 'mp-code-box mp-code-box-dark',
						'style': 'min-height:220px;max-height:420px'
					}, '// modemd AT 调试台就绪。支持实时应答回显。\n\n')
				])
			]),

			/* ---------------- Tab 2: USSD ---------------- */
			E('div', { 'id': 'tool-tab-ussd', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('antenna', 17), 'USSD 业务与话费查询' ])
					]),
					E('div', { 'style': 'max-width:560px;display:flex;flex-direction:column;gap:12px' }, [
						E('p', { 'style': 'font-size:13px;color:var(--mp-neutral-700);margin:0' },
							'USSD 为电路域 (CS) 业务。5G SA 单模数据卡或无 CS 注册的物联网卡可能被基站拒绝。'),
						E('div', { 'style': 'display:flex;gap:8px' }, [
							E('input', {
								'id': 'ussd-code-in',
								'class': 'mp-input',
								'style': 'flex:1',
								'placeholder': '例如 *100# 或 *101#'
							}),
							E('button', {
								'class': 'mp-btn mp-btn-primary',
								'click': ui.createHandlerFn(self, function (btn) {
									var code = (document.getElementById('ussd-code-in').value || '').trim();
									if (!code) return;
									btn.disabled = true;
									btn.textContent = '下发中…';
									var out = document.getElementById('ussd-reply-box');
									out.textContent = '已下发，等待基站返回异步应答 (URC)...';

									task.run(api, 'ussd.send', { modem_id: id, code: code }, { timeout: 30000 })
										.then(function (r) {
											var txt = r && r.result ? (r.result.text || r.result.raw || JSON.stringify(r.result)) : '已收到应答。';
											out.textContent = txt;
										})
										.catch(function (e) {
											out.textContent = '[USSD 失败] ' + (e.message || e);
										})
										.finally(function () {
											btn.disabled = false;
											btn.textContent = '下发 USSD';
										});
								})
							}, '下发 USSD')
						]),
						E('div', { 'style': 'font-size:12.5px;font-weight:600;color:var(--mp-neutral-700);margin-top:6px' }, '运营商回复回显:'),
						E('div', {
							'id': 'ussd-reply-box',
							'class': 'mp-code-box',
							'style': 'min-height:90px'
						}, '暂无 USSD 应答')
					])
				])
			]),

			/* ---------------- Tab 3: 主动搜网 ---------------- */
			E('div', { 'id': 'tool-tab-netscan', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('tower', 17), '可见运营商主动扫描' ])
					]),
					E('div', { 'style': 'display:flex;align-items:center;gap:12px;margin-bottom:14px;flex-wrap:wrap' }, [
						E('label', { 'style': 'font-size:12.5px;font-weight:600' }, '搜网制式:'),
						E('select', { 'id': 'netscan-tech-sel', 'class': 'mp-select', 'style': 'width:120px' }, [
							E('option', { 'value': 'lte' }, '仅 LTE (4G)'),
							E('option', { 'value': 'nr' }, '仅 5G NR'),
							E('option', { 'value': 'both' }, 'LTE + 5G NR')
						]),
						E('label', { 'style': 'font-size:12.5px;font-weight:600' }, '最长等待:'),
						E('select', { 'id': 'netscan-min-sel', 'class': 'mp-select', 'style': 'width:100px' }, [
							E('option', { 'value': '2' }, '2 分钟'),
							E('option', { 'value': '3', 'selected': (true) ? 'selected' : null }, '3 分钟'),
							E('option', { 'value': '5' }, '5 分钟'),
							E('option', { 'value': '8' }, '8 分钟')
						]),
						E('button', {
							'id': 'btn-start-netscan',
							'class': 'mp-btn mp-btn-primary mp-btn-sm',
							'click': ui.createHandlerFn(self, function (btn) {
								var t = document.getElementById('netscan-tech-sel').value;
								var m = parseInt(document.getElementById('netscan-min-sel').value, 10);
								btn.disabled = true;
								btn.textContent = '搜网中… 0s';
								document.getElementById('btn-stop-netscan').disabled = false;

								task.run(api, 'netscan', { modem_id: id, tech: t, minutes: m }, {
									timeout: (m + 1) * 60 * 1000,
									onTick: function (tick) {
										btn.textContent = '搜网中… ' + Math.round((tick.elapsed_ms || 0) / 1000) + 's';
									}
								}).then(function (res) {
									ui.addNotification(null, E('p', {}, '搜网完成'), 'info');
									self.renderNetscanResults(res);
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '搜网结束: ' + (e.message || e)), 'error');
								}).finally(function () {
									btn.disabled = false;
									btn.textContent = '开始搜网';
									document.getElementById('btn-stop-netscan').disabled = true;
								});
							})
						}, '开始搜网'),
						E('button', {
							'id': 'btn-stop-netscan',
							'class': 'mp-btn mp-btn-danger mp-btn-sm',
							'disabled': true,
							'click': ui.createHandlerFn(self, function () {
								api.post('/api/netscan/stop', { modem_id: id }).then(function () {
									ui.addNotification(null, E('p', {}, '已向模组请求中断搜网'), 'info');
								}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								});;
							})
						}, '停止搜网')
					]),
					E('div', { 'id': 'netscan-results-wrap' }, [
						E('p', { 'style': 'color:var(--mp-neutral-500);padding:14px 0' }, '点击「开始搜网」搜索周边可见运营商小区（耗时约 1~3 分钟）。')
					])
				])
			]),

			/* ---------------- Tab 4: 流量配额 ---------------- */
			E('div', { 'id': 'tool-tab-quota', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('gauge', 17), '月度流量配额与超限控制' ]),
						E('span', { 'class': 'mp-badge ' + (quota.enabled ? 'mp-badge-success' : 'mp-badge-muted') }, quota.enabled ? '配额监控中' : '未启用')
					]),
					E('div', { 'style': 'display:grid;grid-template-columns:repeat(auto-fit, minmax(min(220px, 100%), 1fr));gap:12px;margin-bottom:18px' }, [
						metricBox('月度限额', quota.limit_text || (quota.limit_bytes ? fmt.bytes(quota.limit_bytes) : '不限制'), '', '设定阈值'),
						metricBox('本周期已用', quota.used_text || (quota.used_bytes ? fmt.bytes(quota.used_bytes) : '0 B'), '', '累计用量'),
						metricBox('剩余流量', quota.remain_text || (quota.remain_bytes ? fmt.bytes(quota.remain_bytes) : '-'), '', '可用额度'),
						metricBox('超限状态', quota.over_limit ? '已超限' : '正常', '', quota.action === 'block' ? '超限自动断网' : '超限仅告警')
					]),
					E('div', { 'style': 'max-width:540px;display:flex;flex-direction:column;gap:12px' }, [
						E('div', { 'style': 'display:flex;align-items:center;justify-content:space-between' }, [
							E('label', { 'style': 'font-size:13px;font-weight:600' }, '启用月度流量配额保护:'),
							E('input', { 'id': 'quota-enabled', 'type': 'checkbox', 'checked': !!quota.enabled })
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '月度限额 (GB):'),
							E('input', {
								'id': 'quota-limit-gb',
								'class': 'mp-input',
								'style': 'width:100%',
								'type': 'number',
								'step': '0.1',
								'value': quota.limit_bytes ? (quota.limit_bytes / (1024*1024*1024)).toFixed(1) : '10.0'
							})
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '超限执行动作:'),
							E('select', { 'id': 'quota-action', 'class': 'mp-select', 'style': 'width:100%' }, [
								E('option', { 'value': 'block', 'selected': (quota.action === 'block') ? 'selected' : null }, '断开数据连接 (推荐)'),
								E('option', { 'value': 'block_rf', 'selected': (quota.action === 'block_rf') ? 'selected' : null }, '关闭模组射频 (飞行模式)'),
								E('option', { 'value': 'warn', 'selected': (quota.action === 'warn') ? 'selected' : null }, '仅发送告警提醒不阻断')
							])
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '每月重置日 (1~28 日):'),
							E('input', {
								'id': 'quota-reset-day',
								'class': 'mp-input',
								'style': 'width:100%',
								'type': 'number',
								'min': '1',
								'max': '28',
								'value': String(quota.reset_day || 1)
							})
						]),
						E('div', { 'style': 'display:flex;gap:8px;padding-top:8px' }, [
							E('button', {
								'class': 'mp-btn mp-btn-primary',
								'click': ui.createHandlerFn(self, function (b) {
									var en = document.getElementById('quota-enabled').checked;
									var gb = parseFloat(document.getElementById('quota-limit-gb').value);
									var act = document.getElementById('quota-action').value;
									var day = parseInt(document.getElementById('quota-reset-day').value, 10);
									b.disabled = true;

									api.post('/api/quota/set', {
										modem_id: id,
										enabled: en,
										limit_gb: gb,
										action: act,
										reset_day: day,
										count_up: true,
										count_down: true
									}).then(function () {
										ui.addNotification(null, E('p', {}, '配额设置已保存生效'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '保存失败: ' + (e.message || e)), 'error');
										b.disabled = false;
									});
								})
							}, '保存配额设置'),
							E('button', {
								'class': 'mp-btn mp-btn-ghost',
								'click': ui.createHandlerFn(self, function () {
									if (!confirm('确定将本周期的流量计数清零吗？')) return;
									api.post('/api/quota/reset', { modem_id: id }).then(function () {
										ui.addNotification(null, E('p', {}, '本周期用量已清零'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								});;
								})
							}, '本周期用量清零'),
							E('button', {
								'class': 'mp-btn mp-btn-ghost',
								'click': ui.createHandlerFn(self, function () {
									api.post('/api/quota/unblock', { modem_id: id }).then(function () {
										ui.addNotification(null, E('p', {}, '已解除本周期阻断'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								});;
								})
							}, '解除阻断')
						])
					])
				])
			]),

			/* ---------------- Tab 5: USB 网卡模式 ---------------- */
			E('div', { 'id': 'tool-tab-usbnet', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('usb', 17), 'USB 网卡模式配置' ]),
						E('span', { 'class': 'mp-badge mp-badge-primary' }, (usbnet.name || '默认').toUpperCase())
					]),
					E('div', { 'style': 'max-width:540px;display:flex;flex-direction:column;gap:14px' }, [
						E('p', { 'style': 'font-size:13px;color:var(--mp-neutral-700);margin:0' },
							'修改模组工作在 USB 侧暴露的网络适配器接口规范。注意：切模式后需重启模组生效。'),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:6px' }, '选择目标模式:'),
							E('select', { 'id': 'sel-usbnet-mode', 'class': 'mp-select', 'style': 'width:100%' },
								(usbnet.modes || [
									{ value: '0', name: 'RMNET' },
									{ value: '1', name: 'ECM' },
									{ value: '2', name: 'MBIM' },
									{ value: '3', name: 'RNDIS' },
									{ value: '5', name: 'NCM' }
								]).map(function (m) {
									return E('option', { 'value': String(m.value), 'selected': (String(m.value) === String(usbnet.value)) ? 'selected' : null }, m.name + ' (' + m.value + ')');
								})
							)
						]),
						usbnet.note ? E('p', { 'style': 'font-size:12px;color:var(--mp-warning-text);margin:0' }, usbnet.note) : '',
						E('div', {}, [
							E('button', {
								'class': 'mp-btn mp-btn-primary',
								'click': ui.createHandlerFn(self, function (btn) {
									var targetVal = document.getElementById('sel-usbnet-mode').value;
									if (!confirm('确定将 USB 模式切换到档位 ' + targetVal + ' 吗？\n切换后建议重启模组以加载新 USB 描述符。')) return;
									btn.disabled = true;
									api.post('/api/usbnet/set', { modem_id: id, value: targetVal }).then(function (r) {
										ui.addNotification(null, E('p', {}, (r && r.msg) || 'USB 模式已下发写入'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '下发失败: ' + (e.message || e)), 'error');
										btn.disabled = false;
									});
								})
							}, '下发切换模式')
						])
					])
				])
			]),

			/* ---------------- Tab 6: IMEI 管理 ---------------- */
			E('div', { 'id': 'tool-tab-imei', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('chip', 17, 'running'), '模组 IMEI 设备序列号管理' ]),
						E('span', { 'class': 'mp-badge ' + (imei.luhn_ok ? 'mp-badge-success' : 'mp-badge-warning') },
							imei.luhn_ok ? 'Luhn 校验合法' : 'Luhn 异常')
					]),
					E('div', { 'style': 'max-width:540px;display:flex;flex-direction:column;gap:14px' }, [
						E('div', { 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr;margin-bottom:6px' }, [
							rowKv('当前模组 IMEI', imei.imei || '-'),
							rowKv('出厂备份状态', imei.can_restore ? ('已备份 (原值: ' + (imei.original || '-') + ')') : '尚未改写过/无备份'),
							rowKv('改写指令方案', imei.cmd || '厂商私有指令 (AT+EGMR / AT+EGMREXT)')
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '改写为新 IMEI (15 位数字):'),
							E('input', {
								'id': 'imei-new-input',
								'class': 'mp-input',
								'style': 'width:100%;font-family:ui-monospace,monospace',
								'placeholder': '例如 867698040012349'
							})
						]),
						E('p', { 'style': 'font-size:12px;color:var(--mp-neutral-500);line-height:1.5;margin:0' },
							'安全机制：首次改写前系统会自动快照备份模组出厂原始 IMEI 到本地，随时可一键恢复。部分平台改写后需重启模组生效。'),
						E('div', { 'style': 'display:flex;gap:8px;padding-top:4px' }, [
							E('button', {
								'class': 'mp-btn mp-btn-danger',
								'click': ui.createHandlerFn(self, function (btn) {
									var newImei = (document.getElementById('imei-new-input').value || '').trim();
									if (!/^\d{15}$/.test(newImei)) {
										alert('IMEI 必须是严格的 15 位数字');
										return;
									}
									if (!confirm('请确认已知晓改写 IMEI 风险，确定将 IMEI 改写为 ' + newImei + ' 吗？')) return;
									btn.disabled = true;
									/* 改写 IMEI 要「备份 → 写入 → 回读确认」三个串口往返，
									   同步 POST 会撞上 ucode 代理的 20 秒超时 —— 走异步任务。 */
									task.run(api, 'imei.set', { modem_id: id, imei: newImei }, {
										onTick: function (t) {
											if (t && t.progress) btn.textContent = t.progress;
										}
									}).then(function () {
										ui.addNotification(null, E('p', {}, 'IMEI 改写成功'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '改写失败: ' + (e.message || e)), 'error');
										btn.disabled = false;
										btn.textContent = '写入新 IMEI';
									});
								})
							}, '写入新 IMEI'),
							E('button', {
								'class': 'mp-btn mp-btn-ghost',
								'disabled': !imei.can_restore,
								'click': ui.createHandlerFn(self, function (btn) {
									if (!confirm('确定要恢复出厂原始备份的 IMEI 吗？')) return;
									btn.disabled = true;
									task.run(api, 'imei.restore', { modem_id: id }).then(function () {
										ui.addNotification(null, E('p', {}, '原始 IMEI 已恢复'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '恢复失败: ' + (e.message || e)), 'error');
										btn.disabled = false;
									});
								})
							}, '一键恢复出厂备份')
						])
					])
				])
			]),

			/* ---------------- Tab 7: 定时任务 ---------------- */
			E('div', { 'id': 'tool-tab-schedule', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('calendar', 17), '定时任务列表' ]),
						E('div', { 'class': 'mp-card-actions' }, [
							E('button', {
								'class': 'mp-btn mp-btn-primary mp-btn-sm',
								'click': function () { openScheduleModal(); }
							}, [ icons.get('refresh', 12), ' 新建定时任务' ])
						])
					]),
					E('table', { 'class': 'mp-table' }, [
						E('thead', {}, [
							E('tr', {}, [
								E('th', {}, '任务名称'),
								E('th', {}, '执行动作'),
								E('th', {}, '计划时间'),
								E('th', {}, '下次执行'),
								E('th', {}, '状态'),
								E('th', { 'style': 'text-align:right' }, '操作')
							])
						]),
						E('tbody', {}, (schedule.tasks && schedule.tasks.length) ? schedule.tasks.map(function (tsk) {
							return E('tr', {}, [
								E('td', { 'style': 'font-weight:600' }, tsk.name || tsk.id),
								E('td', {}, (tsk.start_actions && tsk.start_actions[0]) ? (tsk.start_actions[0].kind + ' ' + (tsk.start_actions[0].bands || '')) : '-'),
								E('td', {}, tsk.start_time || '-'),
								E('td', { 'style': 'font-size:12px;color:var(--mp-neutral-500)' }, (tsk.next_runs && tsk.next_runs[0]) ? tsk.next_runs[0] : '-'),
								E('td', {}, [
									E('span', { 'class': 'mp-badge ' + (tsk.enabled ? 'mp-badge-success' : 'mp-badge-muted') }, tsk.enabled ? '启用中' : '已停用')
								]),
								E('td', { 'style': 'text-align:right' }, [
									E('div', { 'style': 'display:inline-flex;gap:6px' }, [
										E('button', {
											'class': 'mp-btn mp-btn-ghost mp-btn-sm',
											'click': ui.createHandlerFn(self, function () {
												api.post('/api/schedule/run', { id: tsk.id }).then(function () {
													ui.addNotification(null, E('p', {}, '任务动作已立即执行'), 'info');
												}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								});;
											})
										}, '立即执行'),
										E('button', {
											'class': 'mp-btn mp-btn-ghost mp-btn-sm',
											'click': ui.createHandlerFn(self, function () {
												api.post('/api/schedule/toggle', { id: tsk.id, enabled: !tsk.enabled }).then(function () {
													window.location.reload();
												}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								});;
											})
										}, tsk.enabled ? '停用' : '启用'),
										E('button', {
											'class': 'mp-btn mp-btn-danger mp-btn-sm',
											'click': ui.createHandlerFn(self, function () {
												if (!confirm('确定删除定时任务 ' + tsk.name + ' 吗？')) return;
												api.post('/api/schedule/delete', { id: tsk.id }).then(function () {
													window.location.reload();
												}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								});;
											})
										}, '删除')
									])
								])
							]);
						}) : [
							E('tr', {}, [
								E('td', { 'colspan': 6, 'style': 'text-align:center;color:var(--mp-neutral-500);padding:20px' }, '暂无定时任务，可点击上方「新建定时任务」添加。')
							])
						])
					])
				])
			]),

			/* ---------------- Tab 8: 设备与热插拔 ---------------- */
			E('div', { 'id': 'tool-tab-devices', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('usb', 17), 'USB 模组与串口绑定' ]),
						E('button', {
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'click': ui.createHandlerFn(self, function () {
								api.post('/api/usb/clear').then(function () {
									ui.addNotification(null, E('p', {}, '热插拔事件已清空'), 'info');
									window.location.reload();
								}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
								});;
							})
						}, '清空事件')
					]),
					E('div', { 'style': 'margin-bottom:12px' }, [
						E('div', { 'style': 'font-size:12.5px;font-weight:600;margin-bottom:6px' }, '已检测 USB 设备列表:'),
						E('div', { 'class': 'mp-code-box', 'style': 'max-height:160px' },
							(usbDev.devices && usbDev.devices.length) ? usbDev.devices.map(function(d){ return (d.vid && d.pid ? d.vid + ':' + d.pid : (d.vid_pid || '未知设备'))
									+ ' · ' + (d.product || d.vendor_cn || d.vendor || d.family || 'USB设备')
									+ ' · ' + ((d.drivers && d.drivers.length) ? d.drivers.join(', ') : '无驱动'); }).join('\n') : '暂无 USB 模组设备'
						)
					])
				]),
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('pcie', 17), 'PCIe 模组管理 (MHI / WWAN)' ])
					]),
					E('div', { 'class': 'mp-kv-grid' }, [
						rowKv('PCIe 驱动支持', pcieDev.supported ? '支持' : '内核未提供或无需特殊驱动'),
						rowKv('已检测设备', (pcieDev.devices && pcieDev.devices.length)
						? pcieDev.devices.map(function (d) {
							return d.bdf + (d.pid ? ' [' + d.vid + ':' + d.pid + ']' : '') + (d.driver ? ' 驱动=' + d.driver : '');
					}).join(' / ')
					: '无 PCIe 模组')
					]),
					pcieDev.supported ? E('div', { 'style': 'margin-top:10px' }, [
						E('button', {
							'class': 'mp-btn mp-btn-danger mp-btn-sm',
							'click': ui.createHandlerFn(self, function (btn) {
								if (!confirm('确定执行 PCIe 模组软复位 (remove & rescan) 吗？')) return;
								btn.disabled = true;
								api.post('/api/pcie/reset', {
								bdf: (pcieDev.devices && pcieDev.devices[0]) ? pcieDev.devices[0].bdf : ''
							}).then(function () {
									ui.addNotification(null, E('p', {}, 'PCIe 模组已复位重扫'), 'info');
									window.location.reload();
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '复位失败: ' + (e.message || e)), 'error');
									btn.disabled = false;
								});
							})
						}, 'PCIe 模组软复位')
					]) : ''
				])
			]),

			/* ---------------- Tab 9: 波动记录 ---------------- */
			E('div', { 'id': 'tool-tab-changes', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('pulse', 17), '信号与网络历史波动事件' ]),
						E('button', {
							'class': 'mp-btn mp-btn-danger mp-btn-sm',
							'click': ui.createHandlerFn(self, function () {
								if (!confirm('确定清空所有波动记录吗？')) return;
								api.post('/api/changes/clear', { modem_id: id }).then(function () {
									ui.addNotification(null, E('p', {}, '波动记录已清空'), 'info');
									window.location.reload();
								});
							})
						}, '清空记录')
					]),
					E('table', { 'class': 'mp-table' }, [
						E('thead', {}, [
							E('tr', {}, [
								E('th', { 'style': 'width:150px' }, '记录时间'),
								E('th', { 'style': 'width:140px' }, '监控字段'),
								E('th', {}, '变更前原值'),
								E('th', {}, '变更后新值'),
								E('th', { 'style': 'width:90px' }, '波动幅度')
							])
						]),
						E('tbody', {}, (changes.entries && changes.entries.length) ? changes.entries.map(function (ch) {
							return E('tr', {}, [
								E('td', { 'style': 'font-size:12px;color:var(--mp-neutral-500)' }, ch.at || '-'),
								E('td', { 'style': 'font-weight:600' }, ch.field || '-'),
								E('td', {}, String(ch.from != null ? ch.from : '-')),
								E('td', { 'style': 'font-weight:600' }, String(ch.to != null ? ch.to : '-')),
								E('td', {}, ch.delta ? String(ch.delta) : '-')
							]);
						}) : [
							E('tr', {}, [
								E('td', { 'colspan': 5, 'style': 'text-align:center;color:var(--mp-neutral-500);padding:24px' }, '暂无波动记录（系统在信号或制式跨越阈值变化时会自动记录）')
							])
						])
					])
				])
			]),

			/* ---------------- Tab 10: 运行日志 ---------------- */
			E('div', { 'id': 'tool-tab-logs', 'class': 'mp-tool-pane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [ icons.get('logs', 17), 'modemd 守护进程运行日志' ]),
						E('div', { 'class': 'mp-card-actions' }, [
							E('button', {
								'class': 'mp-btn mp-btn-ghost mp-btn-sm',
								'click': ui.createHandlerFn(self, function (btn) {
									btn.disabled = true;
									api.get('/api/logs', { lines: 200 }).then(function (r) {
										var el = document.getElementById('log-box-text');
										if (el) el.textContent = (r && r.lines && r.lines.length) ? r.lines.join('\n') : '(无日志)';
									}).finally(function () { btn.disabled = false; });
								})
							}, [ icons.get('refresh', 13), ' 刷新日志' ])
						])
					]),
					E('pre', {
						'id': 'log-box-text',
						'class': 'mp-code-box mp-code-box-dark',
						'style': 'min-height:360px;max-height:600px'
					}, (logs && logs.lines && logs.lines.length) ? logs.lines.join('\n') : '暂无日志输出')
				])
			])
		]);

		return root;
	},

	switchTab: function (tabName, clickedBtn) {
		document.querySelectorAll('.mp-subtab-btn').forEach(function (b) { b.classList.remove('active'); });
		if (clickedBtn) clickedBtn.classList.add('active');

		document.querySelectorAll('.mp-tool-pane').forEach(function (p) { p.style.display = 'none'; });
		var target = document.getElementById('tool-tab-' + tabName);
		if (target) target.style.display = 'block';
	},

	renderNetscanResults: function (res) {
		var wrap = document.getElementById('netscan-results-wrap');
		if (!wrap) return;
		wrap.innerHTML = '';

		var ops = (res && res.operators) || [];
		if (!ops.length) {
			wrap.appendChild(E('p', { 'style': 'color:var(--mp-neutral-500);padding:10px 0' }, '本次扫描未发现可见运营商。'));
			return;
		}

		var table = E('table', { 'class': 'mp-table' }, [
			E('thead', {}, [
				E('tr', {}, [
					E('th', {}, '运营商全名'),
					E('th', {}, '简称'),
					E('th', {}, 'PLMN 数字码'),
					E('th', {}, '无线技术 (RAT)'),
					E('th', {}, '网络可见状态')
				])
			]),
			E('tbody', {}, ops.map(function (o) {
				var stMap = { 0: '未知', 1: '当前可用', 2: '当前已驻网', 3: '禁止接入' };
				return E('tr', {}, [
					E('td', { 'style': 'font-weight:600' }, o.long || o.name || '-'),
					E('td', {}, o.short || '-'),
					E('td', { 'style': 'font-family:ui-monospace,monospace' }, o.numeric || '-'),
					E('td', {}, o.tech || o.act || '-'),
					E('td', {}, [
						E('span', { 'class': 'mp-badge ' + (o.status === 2 ? 'mp-badge-success' : (o.status === 1 ? 'mp-badge-primary' : 'mp-badge-muted')) },
							stMap[o.status] || ('状态 ' + o.status))
					])
				]);
			}))
		]);

		wrap.appendChild(table);
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
