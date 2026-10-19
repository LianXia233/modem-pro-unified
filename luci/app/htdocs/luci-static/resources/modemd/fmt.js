'use strict';
'require baseclass';

/* modemd 界面文案、状态映射与格式化工具 (LuCI 侧共享)
 *
 * 规范：
 *   1. 统一术语，与 WebUI 保持一致；
 *   2. 严格区分 null / 0 / 占位符 / 异常，不造假数；
 *   3. 信号等级按 3GPP 真实物理量评级（RSRP / SINR / RSSI）。
 */

var I18N = {
	'zh-cn': {
		'Device Center': '设备中心',
		'Select modem': '选择模组',
		'No modem detected': '未检测到模组',
		'Signal': '信号',
		'Network': '网络与注册',
		'Cells': '基站扫描',
		'Traffic': '实时流量',
		'Dial-up': '拨号上网',
		'Identity': '模组与 SIM 卡信息',
		'Hardware': '硬件状态',
		'Refresh signal': '刷新信号',
		'Refresh network': '刷新网络',
		'Scan cells': '扫描基站',
		'Reading': '读取中…',
		'Running tasks': '进行中的任务',
		'Cancel': '取消',
		'Cancelled': '已取消',
		'Data is %s old': '数据已 %s 旧',
		'Never': '从未',
		'Failed': '失败',
		'Done': '完成',
		'queued': '排队中',
		'running': '进行中',
		'done': '完成',
		'failed': '失败',
		'cancelled': '已取消',
		'Not registered': '未注册',
		'Registered': '已注册',
		'Roaming': '漫游中',
		'Airplane mode': '飞行模式',
		'APN': '接入点 APN',
		'IPv4': 'IPv4 地址',
		'IPv6': 'IPv6 地址',
		'DNS': 'DNS 服务器',
		'MTU': 'MTU',
		'Interface': '网络接口',
		'Operator': '运营商',
		'Band': '频段',
		'RSRP': 'RSRP (信号强度)',
		'RSRQ': 'RSRQ (信号质量)',
		'SINR': 'SINR (信噪比)',
		'RSSI': 'RSSI (接收信号强度)',
		'BER': 'BER (误码率)',
		'Serving cell': '服务小区',
		'Neighbour cells': '邻区列表',
		'Lock bands': '锁频段',
		'Lock cells': '锁基站/小区',
		'Apply': '应用',
		'Unlock all': '全部解除锁定',
		'Locked bands': '已锁定频段',
		'Send SMS': '发送短信',
		'Recipient': '收件人号码',
		'Content': '短信内容',
		'Delete': '删除',
		'Delete all': '全部清空',
		'SMS list is empty': '没有短信',
		'AT console': 'AT 调试台',
		'Send': '发送',
		'Response': '应答回显',
		'USSD code': 'USSD 码',
		'Network scan': '主动搜网',
		'Technology': '接入制式',
		'Minutes': '分钟',
		'Start scan': '开始搜网',
		'Stop scan': '停止搜网',
		'Operators': '运营商列表',
		'Status': '状态',
		'SIM': 'SIM 卡',
		'Unsupported on this modem': '本机不支持',
		'Saving…': '保存中…',
		'Submitting…': '提交中…',
		'Reboot module': '重启模组',
		'Sync clock': '同步时钟',
		'Connected': '已连接',
		'Dialing': '拨号中',
		'Idle': '未拨号',
		'Disconnected': '已断开',
		'Traffic quota': '流量配额',
		'USB net mode': 'USB 网卡模式',
		'IMEI management': 'IMEI 管理',
		'Timers & Schedule': '定时任务',
		'Hotplug & Devices': '设备与热插拔',
		'Fluctuations': '波动记录',
		'Daemon logs': '运行日志',
		'WeChat Bot': '企业微信推送'
	},
	'zh-tw': {
		'Device Center': '裝置中心',
		'Select modem': '選擇模組',
		'No modem detected': '未偵測到模組',
		'Signal': '訊號',
		'Network': '網路與註冊',
		'Cells': '基地台掃描',
		'Traffic': '即時流量',
		'Dial-up': '撥號上網',
		'Identity': '模組與 SIM 卡資訊',
		'Hardware': '硬體狀態',
		'Refresh signal': '重新整理訊號',
		'Refresh network': '重新整理網路',
		'Scan cells': '掃描基地台',
		'Reading': '讀取中…',
		'Running tasks': '進行中的任務',
		'Cancel': '取消',
		'Cancelled': '已取消',
		'Data is %s old': '資料已 %s 舊',
		'Never': '從未',
		'Failed': '失敗',
		'Done': '完成',
		'Not registered': '未註冊',
		'Registered': '已註冊',
		'Roaming': '漫遊中',
		'Airplane mode': '飛航模式',
		'APN': '存取點 APN',
		'Interface': '介面',
		'Operator': '電信業者',
		'Band': '頻段',
		'Serving cell': '服務小區',
		'Neighbour cells': '鄰區清單',
		'Lock bands': '鎖頻段',
		'Lock cells': '鎖基地台/小區',
		'Apply': '套用',
		'Unlock all': '全部解除鎖定',
		'Send SMS': '傳送簡訊',
		'Recipient': '收件人號碼',
		'Content': '簡訊內容',
		'Delete': '刪除',
		'Delete all': '全部清空',
		'SMS list is empty': '沒有簡訊',
		'AT console': 'AT 除錯台',
		'Send': '傳送',
		'Response': '應答回顯',
		'USSD code': 'USSD 碼',
		'Network scan': '主動搜網',
		'Technology': '接入制式',
		'Minutes': '分鐘',
		'Start scan': '開始搜網',
		'Stop scan': '停止搜網',
		'Operators': '電信業者清單',
		'Status': '狀態',
		'SIM': 'SIM 卡',
		'Unsupported on this modem': '本機不支援',
		'Saving…': '儲存中…',
		'Submitting…': '提交中…',
		'Reboot module': '重啟模組',
		'Sync clock': '校準時鐘'
	}
};

function lang() {
	var l = String(document.documentElement.lang || navigator.language || 'en');
	l = l.toLowerCase().replace('_', '-');
	return (l === '' || l === 'auto') ? 'zh-cn' : l;
}

return baseclass.extend({
	__name__: 'modemd.fmt',

	T: function (key) {
		var l = lang();
		var tbl = I18N[l] || I18N[l.split('-')[0]] || I18N['zh-cn'];
		return (tbl && tbl[key] != null) ? tbl[key] : key;
	},

	esc: function (s) {
		return String(s == null ? '' : s)
			.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
			.replace(/"/g, '&quot;').replace(/'/g, '&#39;');
	},

	ago: function (ms) {
		if (ms == null) return this.T('Never');
		var s = Math.max(1, Math.round(ms / 1000));
		if (s < 90) return s + '秒前';
		if (s < 5400) return Math.round(s / 60) + '分钟前';
		return Math.round(s / 3600) + '小时前';
	},

	bytes: function (n) {
		if (n == null || isNaN(n) || n < 0) return '-';
		var u = ['B', 'KB', 'MB', 'GB', 'TB'], i = 0, v = Number(n);
		while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
		return (i === 0 ? v : v.toFixed(v >= 100 ? 1 : 2)) + ' ' + u[i];
	},

	rate: function (bps) {
		if (bps == null || isNaN(bps) || bps < 0) return '-';
		if (bps === 0) return '0 B/s';
		return this.bytes(bps) + '/s';
	},

	bpsRate: function (bps) {
		if (bps == null || isNaN(bps) || bps < 0) return '-';
		if (bps === 0) return '0 bps';
		var u = ['bps', 'Kbps', 'Mbps', 'Gbps'], i = 0, v = Number(bps);
		while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
		return (i === 0 ? v : v.toFixed(v >= 100 ? 1 : 2)) + ' ' + u[i];
	},

	bars: function (rssi) {
		if (rssi == null || isNaN(rssi) || rssi === 0) return 0;
		if (rssi >= -65) return 5;
		if (rssi >= -75) return 4;
		if (rssi >= -85) return 3;
		if (rssi >= -95) return 2;
		if (rssi >= -109) return 1;
		return 0;
	},

	rssiText: function (rssi) {
		if (rssi == null || isNaN(rssi) || rssi === 0) return '-';
		return rssi + ' dBm';
	},

	/* 3GPP 信号评级 (极好/良好/一般/较弱/很差) */
	signalGrade: function (rssi, rsrp, sinr) {
		if (rsrp != null && !isNaN(rsrp)) {
			if (rsrp >= -80) return { text: '极好', level: 'great', color: '#10b981', bars: 5 };
			if (rsrp >= -90) return { text: '良好', level: 'good', color: '#06b6d4', bars: 4 };
			if (rsrp >= -100) return { text: '普通', level: 'fair', color: '#f59e0b', bars: 3 };
			if (rsrp >= -112) return { text: '较弱', level: 'poor', color: '#f97316', bars: 2 };
			return { text: '很差', level: 'poor', color: '#ef4444', bars: 1 };
		}
		if (rssi != null && !isNaN(rssi) && rssi !== 0) {
			var b = this.bars(rssi);
			if (b >= 4) return { text: '良好', level: 'good', color: '#10b981', bars: b };
			if (b === 3) return { text: '普通', level: 'fair', color: '#06b6d4', bars: b };
			if (b === 2) return { text: '较弱', level: 'poor', color: '#f59e0b', bars: b };
			return { text: '很差', level: 'poor', color: '#ef4444', bars: b };
		}
		return { text: '无信号', level: 'none', color: '#94a3b8', bars: 0 };
	},

	regText: function (reg) {
		if (!reg) return '-';
		if (typeof reg === 'string') return reg;
		if (reg.roaming) return this.T('Roaming');
		return reg.registered ? this.T('Registered') : this.T('Not registered');
	},

	dash: function (v) {
		return (v == null || v === '' || v === '-') ? '-' : String(v);
	},

	time: function (iso) {
		if (!iso) return '-';
		var d = new Date(iso);
		if (isNaN(d.getTime())) return iso;
		return ('0' + d.getHours()).slice(-2) + ':' + ('0' + d.getMinutes()).slice(-2) + ':' + ('0' + d.getSeconds()).slice(-2);
	},

	dateTime: function (iso) {
		if (!iso) return '-';
		var d = new Date(iso);
		if (isNaN(d.getTime())) return iso;
		return (d.getFullYear() + '-' + ('0' + (d.getMonth() + 1)).slice(-2) + '-' + ('0' + d.getDate()).slice(-2) + ' ' +
			('0' + d.getHours()).slice(-2) + ':' + ('0' + d.getMinutes()).slice(-2) + ':' + ('0' + d.getSeconds()).slice(-2));
	},

	formatDateTime: function (iso) {
		return this.dateTime(iso);
	},

	ensureCss: function () {
		if (typeof document === 'undefined') return;
		if (!document.getElementById('modemd-glass-css')) {
			var link = document.createElement('link');
			link.id = 'modemd-glass-css';
			link.rel = 'stylesheet';
			link.type = 'text/css';
			link.href = (typeof L !== 'undefined' && L.resource) ? L.resource('modemd/modemd.css') : '/luci-static/resources/modemd/modemd.css';
			document.head.appendChild(link);
		}
		if (typeof window !== 'undefined' && window.E && !window.E._patched_innerHTML) {
			var origE = window.E;
			window.E = function (name, attr, children) {
				var innerHtmlVal = null;
				if (attr && typeof attr === 'object' && 'innerHTML' in attr) {
					innerHtmlVal = attr.innerHTML;
					delete attr.innerHTML;
				}
				var node = origE.apply(this, arguments);
				if (innerHtmlVal != null && node) {
					node.innerHTML = innerHtmlVal;
				}
				return node;
			};
			window.E._patched_innerHTML = true;
		}
		if (typeof L !== 'undefined' && L.dom && !L.dom._patched_innerHTML) {
			L.dom._patched_innerHTML = true;
			var origAttr = L.dom.attr;
			if (typeof origAttr === 'function') {
				L.dom.attr = function (node, key, val) {
					if (node && typeof key === 'object' && key !== null && 'innerHTML' in key) {
						var raw = key.innerHTML;
						delete key.innerHTML;
						if (raw != null) node.innerHTML = raw;
					}
					return origAttr.apply(this, arguments);
				};
			}
		}
	},

	/* 顶部可左右滑动的 TAB 导航栏（二级菜单关联） */
	renderTabBar: function (activeTabId, port, onTabClick) {
		var tabs = [
			{ id: 'overview', name: '设备中心', icon: '<path d="M12 2v20M8 22l4-15 4 15M6 16h12M9 10h6"/><circle cx="12" cy="4" r="2" fill="currentColor"/><path d="M5 4a10 10 0 0 1 14 0"/>' },
			{ id: 'dial',     name: '拨号上网', icon: '<circle cx="6" cy="18" r="3"/><circle cx="18" cy="6" r="3"/><circle cx="18" cy="18" r="3"/><path d="M8.5 16.5l7-7M9 18h6M18 9v6"/>' },
			{ id: 'cells',    name: '基站扫描', icon: '<path d="M2 12h20M12 2v10M7 8l5-6 5 6M9 15a3 3 0 0 0 6 0M6 18a6 6 0 0 0 12 0"/>' },
			{ id: 'lock',     name: '锁网',     icon: '<rect x="4" y="10" width="16" height="11" rx="2"/><path d="M7 10V7a5 5 0 0 1 10 0v3"/>' },
			{ id: 'sms',      name: '短信',     icon: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>' },
			{ id: 'tools',    name: '调试工具', icon: '<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>' },
			{ id: 'modules',  name: '支持模块', icon: '<polygon points="12 2 2 7 12 12 22 7 12 2"/><polyline points="2 17 12 22 22 17"/><polyline points="2 12 12 17 22 12"/>' },
			{ id: 'service',  name: '服务控制', icon: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>' }
		];

		var bar = document.createElement('div');
		bar.className = 'mp-tab-bar';
		bar.id = 'mp-sliding-tab-bar';

		var activeTabEl = null;

		tabs.forEach(function (t) {
			var isActive = (t.id === activeTabId);
			var a = document.createElement('a');
			a.className = 'mp-tab-item' + (isActive ? ' active' : '');
			a.setAttribute('data-tab-id', t.id);
			a.href = (typeof L !== 'undefined' && L.url) ? L.url('admin/modemd/' + t.id) : ('/cgi-bin/luci/admin/modemd/' + t.id);
			a.onclick = function (ev) {
				if (onTabClick) {
					ev.preventDefault();
					onTabClick(t.id);
				}
			};
			a.innerHTML = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" style="flex-shrink:0;">' + t.icon + '</svg><span>' + t.name + '</span>';
			if (isActive) activeTabEl = a;
			bar.appendChild(a);
		});

		// 鼠标滚轮水平滑动
		bar.addEventListener('wheel', function (ev) {
			if (ev.deltaY) {
				bar.scrollLeft += ev.deltaY * 0.8;
				ev.preventDefault();
			}
		}, { passive: false });

		var arrowLeft = document.createElement('button');
		arrowLeft.className = 'mp-tab-arrow';
		arrowLeft.title = '向左滑动';
		arrowLeft.type = 'button';
		arrowLeft.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="15 18 9 12 15 6"/></svg>';
		arrowLeft.onclick = function () { bar.scrollBy({ left: -160, behavior: 'smooth' }); };

		var arrowRight = document.createElement('button');
		arrowRight.className = 'mp-tab-arrow';
		arrowRight.title = '向右滑动';
		arrowRight.type = 'button';
		arrowRight.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="9 18 15 12 9 6"/></svg>';
		arrowRight.onclick = function () { bar.scrollBy({ left: 160, behavior: 'smooth' }); };

		var wrap = document.createElement('div');
		wrap.className = 'mp-tab-bar-wrap';
		wrap.id = 'mp-tab-bar-wrap';
		wrap.appendChild(arrowLeft);
		wrap.appendChild(bar);
		wrap.appendChild(arrowRight);

		var p = port || '9527';
		var btnWeb = document.createElement('a');
		btnWeb.className = 'mp-btn mp-btn-default';
		btnWeb.href = 'http://' + (typeof window !== 'undefined' ? window.location.hostname : '192.168.10.1') + ':' + p;
		btnWeb.target = '_blank';
		btnWeb.title = '在新窗口打开原生内置 WebUI（端口 :' + p + '）';
		btnWeb.style.cssText = 'flex-shrink:0;padding:5px 12px;font-size:12px;border-radius:18px;margin-left:4px;';
		btnWeb.innerHTML = '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="margin-right:4px;"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14L21 3"/></svg><span>WebUI ↗</span>';
		wrap.appendChild(btnWeb);

		if (activeTabEl) {
			window.requestAnimationFrame(function () {
				try { activeTabEl.scrollIntoView({ inline: 'center', block: 'nearest', behavior: 'smooth' }); } catch (e) {}
			});
		}

		return wrap;
	}
});
