'use strict';
'require view';
'require fs';
'require rpc';
'require poll';
'require ui';

/* JS LuCI（OpenWrt 21.02+）服务控制页：启停/重启/自启 + 刷新间隔。 */

/* ---- 界面语言 ----
   dispatcher 会把解析后的语言写进 <html lang="...">（uci luci.main.lang 为 auto 时
   按浏览器 Accept-Language 解析，见 dispatcher.uc 的 determine_request_language）。
   这里跟它走；拿不到就退回浏览器语言，再不行算英文。
   查不到对应翻译时返回英文原文 —— 源码即英文，界面默认语言就是英文。
   两个视图各自带一份（刻意不搞共享模块：固件自带应用也都是单文件视图，
   省掉 class 加载机制的依赖，坏不了）。 */
function modemdLang() {
	var l = String(document.documentElement.lang || navigator.language || 'en');
	l = l.toLowerCase().replace('_', '-');
	return (l === '' || l === 'auto') ? 'en' : l;
}

/* 翻译表：键 = 英文原文，只收本页用到的字符串。 */
var I18N = {
	'zh-cn': {
		'5G Terminal': '5G终端',
		'Service Control': '服务控制',
		'Start or stop the modemd daemon, or change whether it starts at boot.':
			'这里可以启停 modemd 服务，或改它是否随开机自启。',
		'Service': '服务',
		'Enabled at boot': '已设开机自启',
		'Not enabled at boot': '未设开机自启',
		'Running': '运行中',
		'Stopped': '未运行',
		'Web UI port': '网页端口',
		'Start': '启动',
		'Stop': '停止',
		'Restart': '重启',
		'Enable at boot': '设为开机自启',
		'Disable at boot': '取消开机自启',
		'Refresh intervals': '刷新间隔',
		'Push interval 0 disables periodic signal and network updates. Network interval applies while periodic updates are enabled.':
			'信号间隔设为 0 会关闭周期信号和网络更新；网络间隔在周期更新开启时生效。',
		'Signal push interval (seconds)': '信号推送间隔（秒）',
		'Network/operator interval (seconds)': '网络/运营商状态间隔（秒）',
		'Upload/download speed interval (seconds)': '上下行速率采样间隔（秒）',
		'Save intervals and restart': '保存并重启服务',
		'Intervals saved and service restarted.': '间隔已保存，服务已重启。',
		'Failed to save intervals: ': '保存间隔失败：',
		'Action failed: ': '操作失败：'
	},
	'zh-tw': {
		'5G Terminal': '5G終端',
		'Service Control': '服務控制',
		'Start or stop the modemd daemon, or change whether it starts at boot.':
			'這裡可以啟停 modemd 服務，或改它是否隨開機自啟。',
		'Service': '服務',
		'Enabled at boot': '已設開機自啟',
		'Not enabled at boot': '未設開機自啟',
		'Running': '執行中',
		'Stopped': '未執行',
		'Web UI port': '網頁連接埠',
		'Start': '啟動',
		'Stop': '停止',
		'Restart': '重新啟動',
		'Enable at boot': '設為開機自啟',
		'Disable at boot': '取消開機自啟',
		'Refresh intervals': '重新整理間隔',
		'Push interval 0 disables periodic signal and network updates. Network interval applies while periodic updates are enabled.':
			'訊號間隔設為 0 會關閉週期訊號與網路更新；網路間隔在週期更新開啟時生效。',
		'Signal push interval (seconds)': '訊號推送間隔（秒）',
		'Network/operator interval (seconds)': '網路/電信商狀態間隔（秒）',
		'Upload/download speed interval (seconds)': '上下行速率取樣間隔（秒）',
		'Save intervals and restart': '儲存並重新啟動服務',
		'Intervals saved and service restarted.': '間隔已儲存，服務已重新啟動。',
		'Failed to save intervals: ': '儲存間隔失敗：',
		'Action failed: ': '操作失敗：'
	}
};

function T(key) {
	var lang = modemdLang();
	var tbl = I18N[lang] || I18N[lang.split('-')[0]];
	return (tbl && tbl[key] != null) ? tbl[key] : key;
}

var CONFIG = '/etc/modemd/config.toml';

/* ⚠️ rpc.declare 的 expect 是「单键选择器」而非校验器（见 rpc.js 的
   handleCallReply：ret = ret[expect 的第一个键]，取不到或类型不符就用默认值顶上）。
   这两个 ubus 方法的返回都是**按名字建键的对象**，写 expect: {result: []} 会把
   返回削成空数组（自启态就永远显示「未设」），所以一律不写 expect，自己解析。 */
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

/* luci.getInitList → { modemd: { index, stop, enabled, ... }, ... } */
function initEntry(list) {
	return (list && typeof list === 'object') ? (list.modemd || {}) : {};
}

/* procd service list → { modemd: { instances: { instance1: { running, pid } } }, ... }
   运行态只在这里有：luci.getInitList 不给 running。任一实例在跑即算在跑；
   ubus 调不通时按「未运行」显示，别把页面搞崩。 */
function isRunning(list) {
	var inst = list && list.modemd && list.modemd.instances;
	if (!inst) return false;
	for (var k in inst)
		if (inst[k] && inst[k].running) return true;
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

/* 只在 [server] 段内替换/追加整数值（保守：找不到键则追加到该段末尾）。 */
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
		return Promise.all([
			fs.read(CONFIG).catch(function () { return ''; }),
			callInitList(),
			safeServiceList()
		]);
	},

	render: function (data) {
		var text = data[0] || '';
		var runEl = E('span', {}, isRunning(data[2]) ? T('Running') : T('Stopped'));
		var stateEl = E('em', {}, initEntry(data[1]).enabled ? T('Enabled at boot') : T('Not enabled at boot'));
		var self = this;

		poll.add(function () {
			return Promise.all([
				callInitList(),
				safeServiceList()
			]).then(function (r) {
				runEl.textContent = isRunning(r[1]) ? T('Running') : T('Stopped');
				stateEl.textContent = initEntry(r[0]).enabled ? T('Enabled at boot') : T('Not enabled at boot');
			});
		}, 3);

		function act(action) {
			return callInitAction('modemd', action).then(function () {
				window.setTimeout(function () { location.reload(); }, 800);
			}, function (e) {
				ui.addNotification(null, E('p', {}, T('Action failed: ') + action + ' — ' + e), 'error');
			});
		}

		return E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, T('5G Terminal') + ' · ' + T('Service Control')),
			E('div', { 'class': 'cbi-map-descr' },
				T('Start or stop the modemd daemon, or change whether it starts at boot.')),

			E('table', { 'class': 'table' }, [
				E('tr', { 'class': 'tr' }, [
					E('th', { 'class': 'th' }, T('Service')),
					E('td', { 'class': 'td' }, [ runEl, ' · ', stateEl ])
				]),
				E('tr', { 'class': 'tr' }, [
					E('th', { 'class': 'th' }, T('Web UI port')),
					E('td', { 'class': 'td' }, parsePort(text))
				])
			]),

			E('div', { 'class': 'cbi-section-actions' }, [
				E('button', { 'class': 'btn cbi-button cbi-button-apply', 'click': ui.createHandlerFn(self, act, 'start') }, T('Start')),
				' ',
				E('button', { 'class': 'btn cbi-button cbi-button-reset', 'click': ui.createHandlerFn(self, act, 'stop') }, T('Stop')),
				' ',
				E('button', { 'class': 'btn cbi-button cbi-button-action', 'click': ui.createHandlerFn(self, act, 'restart') }, T('Restart')),
				' ',
				E('button', { 'class': 'btn cbi-button', 'click': ui.createHandlerFn(self, act, 'enable') }, T('Enable at boot')),
				' ',
				E('button', { 'class': 'btn cbi-button', 'click': ui.createHandlerFn(self, act, 'disable') }, T('Disable at boot'))
			]),

			E('h3', {}, T('Refresh intervals')),
			E('div', { 'class': 'cbi-map-descr' },
				T('Push interval 0 disables periodic signal and network updates. Network interval applies while periodic updates are enabled.')),

			this.renderInterval('push_interval', T('Signal push interval (seconds)'), 0, 60, text),
			this.renderInterval('network_interval', T('Network/operator interval (seconds)'), 1, 3600, text),
			this.renderInterval('traffic_interval', T('Upload/download speed interval (seconds)'), 1, 60, text),

			E('div', { 'class': 'cbi-section-actions' }, E('button', {
				'class': 'btn cbi-button cbi-button-apply',
				'click': ui.createHandlerFn(self, function () {
					var vals = {};
					['push_interval', 'network_interval', 'traffic_interval'].forEach(function (k) {
						var el = document.getElementById('modemd-' + k);
						vals[k] = parseInt(el.value, 10);
					});
					var next = text;
					['push_interval', 'network_interval', 'traffic_interval'].forEach(function (k) {
						next = writeInterval(next, k, vals[k]);
					});
					return fs.write(CONFIG, next).then(function () {
						return callInitAction('modemd', 'restart');
					}).then(function () {
						ui.addNotification(null, E('p', {}, T('Intervals saved and service restarted.')), 'info');
					}).catch(function (e) {
						ui.addNotification(null, E('p', {}, T('Failed to save intervals: ') + e), 'error');
					});
				})
			}, T('Save intervals and restart')))
		]);
	},

	renderInterval: function (key, label, min, max, text) {
		return E('div', { 'class': 'cbi-value' }, [
			E('label', { 'class': 'cbi-value-title', 'for': 'modemd-' + key }, label),
			E('div', { 'class': 'cbi-value-field' }, E('input', {
				'class': 'cbi-input-text', 'type': 'number',
				'id': 'modemd-' + key, 'min': min, 'max': max,
				'value': parseInterval(text, key, min || 1)
			}))
		]);
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
