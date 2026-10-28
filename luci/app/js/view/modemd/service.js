'use strict';
'require view';
'require fs';
'require rpc';
'require poll';
'require ui';
'require modemd.api as api';
'require modemd.fmt as fmt';
'require modemd.icons as icons';

/* 5G 终端 · 服务控制：
 *   1. Procd 服务启停、重启、开机自启状态控制；
 *   2. /etc/modemd/config.toml 配置管理与后台刷新采样间隔；
 *   3. 内置 SNTP 全局时间同步状态、立即校准与时区动态设置。
 */

var CONFIG = '/etc/modemd/config.toml';

var callInitList = rpc.declare({
	object: 'luci', method: 'getInitList'
});

var callInitAction = rpc.declare({
	object: 'luci', method: 'setInitAction',
	params: [ 'name', 'action' ],
	reject: true
});

var callServiceList = rpc.declare({
	object: 'service', method: 'list'
});

function initEntry(list) {
	return (list && typeof list === 'object') ? (list.modemd || {}) : {};
}

function isRunning(list) {
	var inst = list && list.modemd && list.modemd.instances;
	if (!inst) return false;
	for (var k in inst) {
		if (inst[k] && inst[k].running) return true;
	}
	return false;
}

function safeServiceList() {
	return callServiceList().catch(function () { return null; });
}

function parsePort(text) {
	var m = /\[server\][\s\S]*?listen\s*=\s*["']?[^"'\n]*:(\d+)/.exec(text || '');
	return m ? m[1] : '9527';
}

function parseInterval(text, key, fallback) {
	var re = new RegExp('^\\s*' + key + '\\s*=\\s*(-?\\d+)', 'm');
	var m = re.exec(text || '');
	return m ? parseInt(m[1], 10) : fallback;
}

function writeInterval(text, key, value) {
	var lines = String(text || '').split(/\r?\n/);
	var inServer = false, inserted = false, out = [];

	for (var i = 0; i < lines.length; i++) {
		var line = lines[i];
		var sec = /^\s*\[([^\]]+)\]\s*$/.exec(line);
		if (sec) {
			if (inServer && !inserted) { out.push(key + ' = ' + value); inserted = true; }
			inServer = (sec[1] === 'server');
		}
		if (inServer && !sec && new RegExp('^\\s*' + key + '\\s*=').test(line)) {
			out.push(key + ' = ' + value);
			inserted = true;
			continue;
		}
		out.push(line);
	}
	if (inServer && !inserted) out.push(key + ' = ' + value);
	return out.join('\n').replace(/\n*$/, '\n');
}

return view.extend({
	load: function () {
		fmt.ensureCss();
		return Promise.all([
			fs.read(CONFIG).catch(function () { return ''; }),
			callInitList().catch(function () { return {}; }),
			safeServiceList(),
			api.get('/api/ntp').catch(function () { return null; }),
			/* 系统时区（uci + /etc/TZ）+ 预置列表；与 [ntp] timezone 不是一回事 */
			api.get('/api/timezone').catch(function () { return null; })
		]);
	},

	render: function (data) {
		var self = this;
		var text = data[0] || '';
		var initInfo = data[1] || {};
		var srvInfo = data[2] || {};
		var ntpData = data[3] || null;
		var tzInfo = data[4] || null;

		var running = isRunning(srvInfo);
		var enabled = !!initEntry(initInfo).enabled;

		var srvPill = E('span', {
			'class': 'mp-pill ' + (running ? 'mp-pill-success' : 'mp-pill-danger'),
			'style': 'margin-right:8px;'
		}, running ? '运行中' : '未运行');

		var bootPill = E('span', {
			'class': 'mp-pill ' + (enabled ? 'mp-pill-success' : ''),
			'style': enabled ? '' : 'background:rgba(148,163,184,0.18);color:var(--mp-text-sub);'
		}, enabled ? '已设开机自启' : '未设开机自启');

		var portEl = E('span', { 'style': 'font-family:monospace;font-weight:600;' }, parsePort(text));

		function act(action, label) {
			return callInitAction('modemd', action).then(function () {
				ui.addNotification(null, E('p', {}, '已执行操作: ' + (label || action) + '，正在刷新状态...'), 'info');
				window.setTimeout(function () { location.reload(); }, 900);
			}).catch(function (e) {
				ui.addNotification(null, E('p', {}, '操作失败 (' + action + '): ' + (e.message || e)), 'error');
			});
		}

		/* NTP 时间同步 Card Grid */
		var ntpGrid = E('div', { 'class': 'mp-grid', 'style': 'grid-template-columns:repeat(auto-fit, minmax(min(220px, 100%), 1fr));gap:12px;margin-bottom:12px;' });
		this.renderNtpBoxes(ntpData, ntpGrid);

		/* 状态轮询更新 */
		poll.add(function () {
			return Promise.all([
				callInitList().catch(function () { return {}; }),
				safeServiceList(),
				api.get('/api/ntp').catch(function () { return null; })
			]).then(function (r) {
				var nowRunning = isRunning(r[1]);
				var nowEnabled = !!initEntry(r[0]).enabled;

				srvPill.className = 'mp-pill ' + (nowRunning ? 'mp-pill-success' : 'mp-pill-danger');
				srvPill.textContent = nowRunning ? '运行中' : '未运行';

				bootPill.className = 'mp-pill ' + (nowEnabled ? 'mp-pill-success' : '');
				bootPill.style = nowEnabled ? '' : 'background:rgba(148,163,184,0.18);color:var(--mp-text-sub);';
				bootPill.textContent = nowEnabled ? '已设开机自启' : '未设开机自启';

				if (r[2]) {
					self.renderNtpBoxes(r[2], ntpGrid);
				}
			});
		}, 3);

		/* 服务操作按钮组 */
		var btnStart = E('button', { 'class': 'mp-btn mp-btn-primary', 'click': ui.createHandlerFn(self, act, 'start', '启动') }, [icons.get('check', 14), ' 启动服务']);
		var btnStop = E('button', { 'class': 'mp-btn mp-btn-danger', 'click': ui.createHandlerFn(self, act, 'stop', '停止') }, [icons.get('close', 14), ' 停止服务']);
		var btnRestart = E('button', { 'class': 'mp-btn mp-btn-default', 'click': ui.createHandlerFn(self, act, 'restart', '重启') }, [icons.get('refresh', 14), ' 重启服务']);
		var btnEnable = E('button', { 'class': 'mp-btn mp-btn-default', 'click': ui.createHandlerFn(self, act, 'enable', '设为开机自启') }, '设为开机自启');
		var btnDisable = E('button', { 'class': 'mp-btn mp-btn-default', 'click': ui.createHandlerFn(self, act, 'disable', '取消开机自启') }, '取消开机自启');

		var srvCard = E('div', { 'class': 'mp-card' }, [
			E('div', { 'class': 'mp-card-title', 'style': 'display:flex;align-items:center;gap:8px;' }, [
				icons.get('modem', 18), ' 后台守护进程控制 (modemd procd)'
			]),
			E('div', { 'class': 'mp-grid', 'style': 'grid-template-columns:repeat(auto-fit, minmax(min(220px, 100%), 1fr));gap:12px;margin-bottom:14px;' }, [
				E('div', { 'class': 'mp-metric-box' }, [
					E('div', { 'class': 'mp-metric-label' }, '服务状态 / 开机自启'),
					E('div', { 'class': 'mp-metric-value', 'style': 'display:flex;align-items:center;' }, [ srvPill, bootPill ])
				]),
				E('div', { 'class': 'mp-metric-box' }, [
					E('div', { 'class': 'mp-metric-label' }, 'modemd API 监听端口'),
					E('div', { 'class': 'mp-metric-value' }, [ portEl ])
				])
			]),
			E('div', { 'style': 'display:flex;flex-wrap:wrap;gap:8px;' }, [
				btnStart, btnStop, btnRestart, btnEnable, btnDisable
			])
		]);

		/* NTP 时间同步 Card */

		var btnNtpSync = E('button', { 'class': 'mp-btn mp-btn-primary' }, [icons.get('refresh', 14), ' 立即同步']);
		btnNtpSync.addEventListener('click', function () {
			btnNtpSync.disabled = true;
			api.post('/api/ntp/sync', {}).then(function (d) {
				btnNtpSync.disabled = false;
				ui.addNotification(null, E('p', {}, '已触发 SNTP 立即同步'), 'info');
				if (d) self.renderNtpBoxes(d, ntpGrid);
			}).catch(function (e) {
				btnNtpSync.disabled = false;
				ui.addNotification(null, E('p', {}, '触发 NTP 同步失败: ' + (e.message || e)), 'error');
			});
		});

		/* 时区：预置下拉（原来是 window.prompt 让用户手打 IANA 名，很容易打错）。
		   数据源 /api/timezone：它读写的是**系统**时区（/etc/config/system 的
		   zonename+timezone 与 /etc/TZ），不是 modemd 自己那份 [ntp] timezone。 */
		var tzCur = (tzInfo && tzInfo.current) || {};
		var tzPresets = (tzInfo && tzInfo.presets) || [];
		var tzSel = E('select', { 'class': 'mp-select', 'style': 'min-width:250px' }, []);
		(function fillTz() {
			var have = false;
			tzPresets.forEach(function (p) {
				var sel = p.zonename === tzCur.zonename;
				if (sel) have = true;
				tzSel.appendChild(E('option', { 'value': p.zonename, 'selected': sel ? 'selected' : null },
					p.zonename + '（' + p.label + '）'));
			});
			/* 系统当前时区不在预置列表里 → 保留并置顶显示，绝不自动覆盖。
			   用户可能是用 uci 手工设的，界面只如实展示。 */
			if (!have && tzCur.zonename) {
				tzSel.insertBefore(
					E('option', { 'value': tzCur.zonename, 'selected': 'selected' },
						tzCur.zonename + '（当前值，不在预置列表）'),
					tzSel.firstChild
				);
			}
			if (!tzCur.zonename && tzSel.firstChild) {
				tzSel.firstChild.selected = true;
			}
			/* 一个选项都没有（接口拉失败 / 还没加载完）：别留个空下拉，
			   明确告诉用户「没读到」，比空白好排查。 */
			if (!tzSel.firstChild) {
				tzSel.appendChild(E('option', { 'value': '' }, '（未读到系统时区，点「刷新状态」重试）'));
			}
			/* ⚠️ LuCI 的 `E()` 会把 `selected: false` 照样输出成
			   `selected="false"`，而 HTML 里布尔属性**只看存在性** ——
			   于是所有选项都被判为选中，最后一个赢。实测这台机器的下拉
			   永远停在 Australia/Sydney（预置列表最后一项），而系统时区
			   其实是 Asia/Shanghai。
			   用 `.value` 兜底纠正，不依赖 `selected` 属性是否正确。 */
			if (tzCur.zonename) {
				tzSel.value = tzCur.zonename;
			}
		})();

		var btnNtpTz = E('button', { 'class': 'mp-btn mp-btn-default' }, '应用时区');
		btnNtpTz.addEventListener('click', function () {
			var val = tzSel.value;
			if (!val) return;
			btnNtpTz.disabled = true;
			api.post('/api/timezone', { zonename: val }).then(function (r) {
				var cur = (r && r.current) || {};
				ui.addNotification(null, E('p', {},
					'时区已设为 ' + val + (cur.offset_label ? '（' + cur.offset_label + '）' : '') +
					'，已回读校验'), 'info');
				return Promise.all([
					api.get('/api/ntp').catch(function () { return null; }),
					api.get('/api/timezone').catch(function () { return null; })
				]).then(function (d) {
					if (d[0]) { ntpData = d[0]; self.renderNtpBoxes(d[0], ntpGrid); }
					if (d[1]) tzInfo = d[1];
				});
			}).catch(function (e) {
				ui.addNotification(null, E('p', {}, '更改时区失败: ' + (e.message || e)), 'error');
			}).finally(function () { btnNtpTz.disabled = false; });
		});

		var btnNtpReload = E('button', { 'class': 'mp-btn mp-btn-default' }, [icons.get('refresh', 14), ' 刷新状态']);
		btnNtpReload.addEventListener('click', function () {
			btnNtpReload.disabled = true;
			api.get('/api/ntp').then(function (d) {
				btnNtpReload.disabled = false;
				if (d) { ntpData = d; self.renderNtpBoxes(d, ntpGrid); }
				ui.addNotification(null, E('p', {}, '时间同步状态已更新'), 'info');
			}).catch(function (e) {
				btnNtpReload.disabled = false;
				ui.addNotification(null, E('p', {}, '刷新 NTP 失败: ' + (e.message || e)), 'error');
			});
		});

		var ntpCard = E('div', { 'class': 'mp-card', 'style': 'margin-top:16px;' }, [
			E('div', { 'class': 'mp-card-title', 'style': 'display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:8px;' }, [
				E('div', { 'style': 'display:flex;align-items:center;gap:8px;' }, [
					icons.get('clock', 18), ' 时间同步 (SNTP 全局校时)'
				]),
				/* 时区下拉必须**进 DOM**。
				   早先只把它创建出来、按钮也绑了，却没 append 到任何节点 ——
				   页面上只剩一个「应用时区」按钮，点下去 `tzSel.value` 恒为空、
				   函数第一行就 return，表现为「时区选择功能不可用」。
				   现在和按钮并排放进卡片头，并标出这一项管的是**系统**时区。 */
				E('div', { 'style': 'display:flex;gap:8px;align-items:center;flex-wrap:wrap' }, [
					E('span', { 'style': 'font-size:12px;color:var(--mp-text-sub)' }, '系统时区'),
					tzSel,
					btnNtpTz,
					btnNtpSync,
					btnNtpReload
				])
			]),
			ntpGrid,
			E('div', { 'class': 'mp-desc', 'style': 'font-size:12px;line-height:1.6;color:var(--mp-text-sub);padding-top:4px;' }, [
				'右上角「系统时区」改的是**路由器本身**（/etc/config/system 的 zonename+timezone 与 /etc/TZ），改完立即回读校验；',
				'下方「当前时区」显示的是 **modemd 自己**的时区（[ntp] timezone，决定日志时间戳与定时任务）。两者已在上面的应用动作里同步。',
				'modemd 内置独立 SNTP 客户端，启动 3 秒后首试校准。校准仅对插件内部日志、定时器和流量配额生效，不改变系统底层时钟。',
				'服务器与时区持久化保存在 ', E('code', {}, '/etc/modemd/config.toml'), ' 的 ', E('code', {}, '[ntp]'), ' 段。'
			])
		]);

		/* 刷新间隔设置 Card */
		var inputPush = E('input', {
			'type': 'number',
			'class': 'cbi-input-text',
			'min': 0, 'max': 60,
			'value': parseInterval(text, 'push_interval', 3),
			'style': 'max-width:140px;'
		});

		var inputNetwork = E('input', {
			'type': 'number',
			'class': 'cbi-input-text',
			'min': 1, 'max': 3600,
			'value': parseInterval(text, 'network_interval', 10),
			'style': 'max-width:140px;'
		});

		var inputTraffic = E('input', {
			'type': 'number',
			'class': 'cbi-input-text',
			'min': 1, 'max': 60,
			'value': parseInterval(text, 'traffic_interval', 1),
			'style': 'max-width:140px;'
		});

		var btnSaveIntervals = E('button', {
			'class': 'mp-btn mp-btn-primary',
			'style': 'margin-top:12px;'
		}, [icons.get('check', 14), ' 保存间隔并重启服务']);

		btnSaveIntervals.addEventListener('click', function () {
			btnSaveIntervals.disabled = true;
			var vals = {
				'push_interval': parseInt(inputPush.value, 10),
				'network_interval': parseInt(inputNetwork.value, 10),
				'traffic_interval': parseInt(inputTraffic.value, 10)
			};
			var next = text;
			['push_interval', 'network_interval', 'traffic_interval'].forEach(function (k) {
				next = writeInterval(next, k, vals[k]);
			});

			fs.write(CONFIG, next).then(function () {
				text = next;
				return callInitAction('modemd', 'restart');
			}).then(function () {
				btnSaveIntervals.disabled = false;
				ui.addNotification(null, E('p', {}, '刷新间隔已更新，modemd 服务已自动重启。'), 'info');
			}).catch(function (e) {
				btnSaveIntervals.disabled = false;
				ui.addNotification(null, E('p', {}, '保存间隔失败: ' + (e.message || e)), 'error');
			});
		});

		var intervalsCard = E('div', { 'class': 'mp-card', 'style': 'margin-top:16px;' }, [
			E('div', { 'class': 'mp-card-title', 'style': 'display:flex;align-items:center;gap:8px;' }, [
				icons.get('history', 18), ' 后台状态刷新与采样间隔'
			]),
			E('div', { 'class': 'mp-desc', 'style': 'margin-bottom:12px;font-size:12px;' },
				'信号推送间隔为 0 时将禁用周期性推送；网络与运营商状态采样在启用周期更新时生效。'
			),
			E('table', { 'class': 'mp-table', 'style': 'margin:0;' }, [
				E('tr', {}, [
					E('td', { 'style': 'width:40%;font-weight:600;' }, '信号与状态推送间隔（秒）'),
					E('td', {}, [ inputPush, E('span', { 'class': 'mp-desc', 'style': 'margin-left:8px;' }, '推荐 3 秒，0 表示关闭') ])
				]),
				E('tr', {}, [
					E('td', { 'style': 'font-weight:600;' }, '网络与运营商状态查询间隔（秒）'),
					E('td', {}, [ inputNetwork, E('span', { 'class': 'mp-desc', 'style': 'margin-left:8px;' }, '推荐 10 秒') ])
				]),
				E('tr', {}, [
					E('td', { 'style': 'font-weight:600;' }, '上下行实时速率采样间隔（秒）'),
					E('td', {}, [ inputTraffic, E('span', { 'class': 'mp-desc', 'style': 'margin-left:8px;' }, '推荐 1 秒') ])
				])
			]),
			btnSaveIntervals
		]);

		return E('div', { 'class': 'mp-wrap' }, [
			fmt.renderTabBar('service'),
			E('div', { 'class': 'mp-header' }, [
				E('h2', { 'class': 'mp-title' }, '5G 终端 · 服务控制'),
				E('div', { 'class': 'mp-desc' }, '管理 modemd 守护进程运行状态、开机自启、时间同步服务与状态刷新周期。')
			]),
			srvCard,
			ntpCard,
			intervalsCard
		]);
	},

	renderNtpBoxes: function (d, grid) {
		grid.innerHTML = '';
		if (!d) {
			grid.appendChild(E('div', { 'style': 'padding:14px;color:var(--mp-text-muted);' }, '暂无时间同步状态数据'));
			return;
		}

		function box(label, val, wide) {
			var b = E('div', {
				'class': 'mp-metric-box',
				'style': (wide ? 'grid-column:1 / -1;' : '') + 'padding:10px 14px;'
			}, [
				E('div', { 'class': 'mp-metric-label', 'style': 'font-size:11px;' }, label),
				E('div', { 'class': 'mp-metric-value', 'style': 'font-size:13px;word-break:break-all;' }, val == null || val === '' ? '-' : String(val))
			]);
			grid.appendChild(b);
		}

		if (!d.enabled) {
			box('本机时间', d.now_local || d.now);
			box('时间来源', '设备时间（NTP 未启用，[ntp] enabled=false）');
			return;
		}

		var ok = d.source === 'ntp';
		var phase = (d.phase_label || d.phase || '') + (d.next_in_secs > 0 ? ('，' + d.next_in_secs + 's 后再试') : '，正在尝试');

		box('本机时间', d.now_local || d.now);
		box('时间来源', ok ? ('NTP 校准（' + (d.server || '-') + '）') : '设备时间（尚未同步成功）');
		box('校准偏移', ok ? ((d.offset_secs >= 0 ? '+' : '') + d.offset_secs + ' 秒 · 往返 ' + (d.delay_ms || 0) + 'ms') : '0 秒');
		box('同步阶段', phase);
		box('成功 / 失败', (d.ok_count || 0) + ' / ' + (d.fail_count || 0));
		box('当前时区', d.timezone || '（跟随系统 TZ）');
		box('NTP 服务器', (d.servers || []).join('，') || '-', true);
		if (d.last_error) box('最近错误', d.last_error, true);
		if (d.last_ok_at) box('上次成功', fmt.formatDateTime(d.last_ok_at));
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
