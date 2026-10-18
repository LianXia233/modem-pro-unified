'use strict';
'require view';
'require fs';

/* JS LuCI（OpenWrt 21.02+）概览页：内嵌 modemd 自带网页。
   端口从 /etc/modemd/config.toml 的 [server] listen 里解析（默认 9527）。 */

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
		'Overview': '设备中心',
		'This is modemd\'s own web UI: modem list, signal, cells, SMS, dialing and automatic WAN provisioning.':
			'下面是 modemd 自带的界面：模组列表、信号、小区、短信、拨号、自动组网都在里面。',
		'The daemon is a separate HTTP service on port %s.': '守护进程是独立的 HTTP 服务，端口 %s。',
		'Open in a new window': '在新窗口打开'
	},
	'zh-tw': {
		'5G Terminal': '5G終端',
		'Overview': '設備中心',
		'This is modemd\'s own web UI: modem list, signal, cells, SMS, dialing and automatic WAN provisioning.':
			'下面是 modemd 自帶的介面：模組清單、訊號、小區、簡訊、撥號、自動組網都在裡面。',
		'The daemon is a separate HTTP service on port %s.': '守護行程是獨立的 HTTP 服務，連接埠 %s。',
		'Open in a new window': '另開視窗'
	}
};

function T(key) {
	var lang = modemdLang();
	var tbl = I18N[lang] || I18N[lang.split('-')[0]];
	return (tbl && tbl[key] != null) ? tbl[key] : key;
}

/* 带 %s 的翻译：参数按顺序填进去（填不拢就原样返回，别把页面搞崩）。 */
function TF(key) {
	var s = T(key), i = 1, a = arguments;
	return String(s).replace(/%s/g, function () { return (i < a.length) ? a[i++] : '%s'; });
}

function parsePort(text) {
	var m = /\[server\][\s\S]*?listen\s*=\s*["']?[^"'\n]*:(\d+)/.exec(text || '');
	return m ? m[1] : '9527';
}

function uiURL(text) {
	return 'http://' + (window.location.hostname || '192.168.1.1') + ':' + parsePort(text) + '/';
}

return view.extend({
	load: function () {
		return fs.read('/etc/modemd/config.toml').catch(function () { return ''; });
	},

	render: function (text) {
		var url = uiURL(text);

		return E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, T('5G Terminal') + ' · ' + T('Overview')),
			E('div', { 'class': 'cbi-map-descr' },
				T("This is modemd's own web UI: modem list, signal, cells, SMS, dialing and automatic WAN provisioning.")),
			E('p', {}, [
				TF('The daemon is a separate HTTP service on port %s.', parsePort(text)), ' ',
				E('a', {
					'class': 'btn cbi-button cbi-button-action',
					'href': url, 'target': '_blank', 'rel': 'noopener'
				}, T('Open in a new window'))
			]),
			E('iframe', {
				'src': url,
				'style': 'width:100%;height:78vh;border:1px solid #cfd4da;border-radius:6px;background:#fff'
			})
		]);
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
