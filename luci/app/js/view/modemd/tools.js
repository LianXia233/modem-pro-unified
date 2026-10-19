'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';

/* 调试工具页：AT 调试台 / USSD / 主动扫网。
 *
 * 三件全是慢活，**全部走异步任务**（提交 → 立即返回 → 轮询进度）：
 *   · AT：可取消（后端用 command_cancellable，读到一半也能叫停）；
 *   · USSD：应答是异步 URC，这里显示最近一次；
 *   · 扫网：最长 10 分钟，轮询期间页面随便用。
 */

return view.extend({
	load: function () {
		store.start(api);
		var id = store.id() != null ? store.id() : undefined;
		return Promise.all([
			api.get('/api/ussd', { modem_id: id }),
			api.get('/api/at/presets').catch(function () { return []; })
		]);
	},

	render: function (data) {
		var self = this;
		var ussd = data[0] || {};
		var presets = Array.isArray(data[1]) ? data[1] : [];
		var id = store.id() != null ? store.id() : 0;

		/* ---------- AT 调试台 ---------- */
		var atOut = E('pre', {
			'id': 'at-out',
			'style': 'background:#1b1f24;color:#d5d9de;padding:10px;border-radius:6px;min-height:140px;max-height:340px;overflow:auto;font-size:12px;white-space:pre-wrap'
		}, '');
		var atIn = E('input', { 'class': 'cbi-input-text', 'id': 'at-in', 'placeholder': 'AT+CSQ', 'style': 'flex:1;min-width:200px' });
		var atRun = E('button', { 'class': 'btn cbi-button cbi-button-apply', 'click': ui.createHandlerFn(self, function (btn) {
			var cmd = (atIn.value || '').trim();
			if (!cmd) return Promise.reject(new Error('先输入一条指令'));
			btn.disabled = true;
			atRun.textContent = '发送中…';
			return task.run(api, 'at', { modem_id: id, cmd: cmd }, { timeout: 60000 })
				.then(function (r) {
					var lines = (r && r.resp) || [];
					atOut.textContent += '> ' + cmd + '\n' + (lines.join('\n') || '(空)') + '\n\n';
					atOut.scrollTop = atOut.scrollHeight;
				})
				.catch(function (e) {
					atOut.textContent += '> ' + cmd + '\n[错误] ' + (e.message || e) + '\n\n';
				})
				.finally(function () { btn.disabled = false; btn.textContent = fmt.T('Send'); });
		}) }, fmt.T('Send'));
		atIn.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') atRun.click(); });

		var presetSel = E('select', { 'class': 'cbi-input-select', 'style': 'min-width:220px', 'change': function (ev) {
			if (ev.target.value) atIn.value = ev.target.value;
		} }, [ E('option', { 'value': '' }, '— 常用指令 —') ]);
		presets.forEach(function (p) {
			presetSel.appendChild(E('option', { 'value': p.cmd || p }, p.label ? (p.label + ' · ' + (p.cmd || p)) : (p.cmd || p)));
		});

		/* ---------- USSD ---------- */
		var ussdOut = E('pre', {
			'style': 'background:#f5f5f5;padding:10px;border-radius:6px;min-height:60px;white-space:pre-wrap'
		}, ussd && ussd.result ? (ussd.result.text || ussd.result.raw || JSON.stringify(ussd.result)) : '（无应答）');
		var ussdIn = E('input', { 'class': 'cbi-input-text', 'placeholder': '*100#', 'style': 'width:170px' });
		var ussdBtn = E('button', { 'class': 'btn cbi-button cbi-button-apply', 'click': ui.createHandlerFn(self, function (btn) {
			var code = (ussdIn.value || '').trim();
			if (!code) return Promise.reject(new Error('输入 USSD 码'));
			btn.disabled = true;
			return task.run(api, 'ussd.send', { modem_id: id, code: code }, { timeout: 30000 })
				.then(function (r) {
					var txt = r && r.result ? (r.result.text || r.result.raw || JSON.stringify(r.result)) : '已下发，等待模组应答（异步 URC）';
					ussdOut.textContent = txt;
					if (!r || !r.result)
						window.setTimeout(function () {
							api.get('/api/ussd', { modem_id: id }).then(function (d) {
								if (d && d.result) ussdOut.textContent = d.result.text || d.result.raw || JSON.stringify(d.result);
							});
						}, 4000);
				})
				.catch(function (e) { ui.addNotification(null, E('p', {}, String(e.message || e)), 'error'); })
				.finally(function () { btn.disabled = false; });
		}) }, fmt.T('Send'));

		/* ---------- 主动扫网 ---------- */
		var scanOut = E('div', {}, []);
		var techSel = E('select', { 'style': 'width:110px' }, [
			E('option', { 'value': 'lte' }, 'LTE'), E('option', { 'value': 'nr' }, 'NR'),
			E('option', { 'value': 'both' }, 'LTE+NR')
		]);
		var minSel = E('select', { 'style': 'width:90px' }, [1, 2, 3, 5, 8, 10].map(function (m) {
			return E('option', { 'value': String(m) }, m + ' min');
		}));
		var scanBtn = E('button', { 'class': 'btn cbi-button cbi-button-apply', 'click': ui.createHandlerFn(self, function (btn) {
			btn.disabled = true;
			btn.textContent = '扫网中… 0s';
			return task.run(api, 'netscan', { modem_id: id, tech: techSel.value, minutes: parseInt(minSel.value, 10) }, {
				timeout: 11 * 60 * 1000,
				onTick: function (t) { btn.textContent = '扫网中… ' + Math.round((t.elapsed_ms || 0) / 1000) + 's'; }
			}).then(function (r) {
				scanOut.innerHTML = '';
				scanOut.appendChild(self.renderOperators(r));
				ui.addNotification(null, E('p', {}, '扫网完成'), 'info');
			}).catch(function (e) {
				ui.addNotification(null, E('p', {}, '扫网失败：' + (e.message || e)), 'error');
			}).finally(function () { btn.disabled = false; btn.textContent = fmt.T('Start scan'); });
		}) }, fmt.T('Start scan'));

		return E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '5G 终端 · 调试工具'),
			E('div', { 'class': 'cbi-map-descr' },
				'所有耗时操作都走后台任务：提交后立刻返回，轮询显示进度，可以随时切走。'),

			E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, fmt.T('AT console')),
				E('div', { 'style': 'display:flex;gap:8px;flex-wrap:wrap;margin-bottom:8px' }, [ presetSel ]),
				E('div', { 'style': 'display:flex;gap:8px;flex-wrap:wrap' }, [ atIn, atRun ]),
				atOut
			]),

			E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, 'USSD'),
				E('div', { 'style': 'display:flex;gap:8px;flex-wrap:wrap;margin-bottom:8px' }, [ ussdIn, ussdBtn ]),
				ussdOut
			]),

			E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, fmt.T('Network scan')),
				E('div', { 'style': 'display:flex;gap:8px;flex-wrap:wrap;margin-bottom:8px' }, [
					techSel, minSel, scanBtn
				]),
				scanOut
			])
		]);
	},

	renderOperators: function (r) {
		var ops = (r && r.operators) || [];
		var t = E('table', { 'class': 'table' }, [
			E('tr', { 'class': 'tr table-titles' }, [
				E('th', { 'class': 'th' }, fmt.T('Operator')), E('th', { 'class': 'th' }, 'PLMN'),
				E('th', { 'class': 'th' }, 'RAT'), E('th', { 'class': 'th' }, fmt.T('Status'))
			])
		]);
		ops.forEach(function (o) {
			t.appendChild(E('tr', { 'class': 'tr' }, [
				E('td', { 'class': 'td' }, o.name || o.long || '-'),
				E('td', { 'class': 'td' }, o.plmn || '-'),
				E('td', { 'class': 'td' }, o.rat || o.act || '-'),
				E('td', { 'class': 'td' }, o.status || '-')
			]));
		});
		if (!ops.length)
			t.appendChild(E('tr', { 'class': 'tr' }, [ E('td', { 'class': 'td', 'colspan': 4 }, '（无结果）') ]));
		return t;
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
