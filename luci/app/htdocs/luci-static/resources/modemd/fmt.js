'use strict';
'require baseclass';

/* modemd 界面文案与格式化（LuCI 侧共享）。
 *
 * 语言：dispatcher 把解析后的语言写进 <html lang>（uci luci.main.lang=auto 时按
 * Accept-Language），这里跟它走；查不到翻译返回英文原文。
 * 术语与 modemd 原生 WebUI 对齐（「设备中心 / 拨号上网 / 锁网」…）。
 */

var I18N = {
	'zh-cn': {
		'Select modem': '选择模组',
		'No modem detected': '未检测到模组',
		'Signal': '信号',
		'Network': '网络',
		'Cells': '基站',
		'Traffic': '流量',
		'Dial-up': '拨号上网',
		'Identity': '身份信息',
		'Refresh signal': '刷新信号',
		'Refresh network': '刷新网络信息',
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
		'Roaming': '漫游',
		'Airplane mode': '飞行模式',
		'APN': '接入点 APN',
		'IPv4': 'IPv4 地址',
		'IPv6': 'IPv6 地址',
		'DNS': 'DNS',
		'MTU': 'MTU',
		'Interface': '接口',
		'Operator': '运营商',
		'Band': '频段',
		'RSRP': 'RSRP',
		'RSRQ': 'RSRQ',
		'SINR': 'SINR',
		'RSSI': 'RSSI',
		'Serving cell': '服务小区',
		'Neighbour cells': '邻区',
		'Lock bands': '锁频段',
		'Apply': '应用',
		'Unlock all': '全部解除锁定',
		'Locked bands': '已锁定频段',
		'Send SMS': '发送短信',
		'Recipient': '收件人',
		'Content': '内容',
		'Delete': '删除',
		'SMS list is empty': '没有短信',
		'AT console': 'AT 调试台',
		'Send': '发送',
		'Response': '应答',
		'USSD code': 'USSD 码',
		'Network scan': '主动扫网',
		'Technology': '制式',
		'Minutes': '分钟',
		'Start scan': '开始扫描',
		'Stop scan': '停止扫描',
		'Operators': '运营商列表',
		'Status': '状态',
		'SIM': 'SIM 卡',
		'Unsupported on this modem': '本机不支持',
		'Saving…': '保存中…',
		'Submitting…': '提交中…'
	},
	'zh-tw': {
		'Select modem': '選擇模組',
		'No modem detected': '未偵測到模組',
		'Signal': '訊號',
		'Network': '網路',
		'Cells': '基地台',
		'Traffic': '流量',
		'Dial-up': '撥號上網',
		'Identity': '身分資訊',
		'Refresh signal': '重新整理訊號',
		'Refresh network': '重新整理網路資訊',
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
		'Roaming': '漫遊',
		'Airplane mode': '飛航模式',
		'Interface': '介面',
		'Operator': '電信業者',
		'Band': '頻段',
		'Serving cell': '服務小區',
		'Neighbour cells': '鄰區',
		'Lock bands': '鎖頻段',
		'Apply': '套用',
		'Unlock all': '全部解除鎖定',
		'Send SMS': '傳送簡訊',
		'Recipient': '收件人',
		'Content': '內容',
		'Delete': '刪除',
		'SMS list is empty': '沒有簡訊',
		'AT console': 'AT 除錯台',
		'Send': '傳送',
		'Response': '應答',
		'USSD code': 'USSD 碼',
		'Network scan': '主動掃網',
		'Technology': '制式',
		'Minutes': '分鐘',
		'Start scan': '開始掃描',
		'Stop scan': '停止掃描',
		'Operators': '電信業者清單',
		'Status': '狀態',
		'SIM': 'SIM 卡',
		'Unsupported on this modem': '本機不支援',
		'Saving…': '儲存中…',
		'Submitting…': '提交中…'
	}
};

function lang() {
	var l = String(document.documentElement.lang || navigator.language || 'en');
	l = l.toLowerCase().replace('_', '-');
	return (l === '' || l === 'auto') ? 'en' : l;
}

return baseclass.extend({
	__name__: 'modemd.fmt',

	/* 翻译（查不到返回英文原文） */
	T: function (key) {
		var l = lang();
		var tbl = I18N[l] || I18N[l.split('-')[0]];
		return (tbl && tbl[key] != null) ? tbl[key] : key;
	},

	esc: function (s) {
		return String(s == null ? '' : s)
			.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
			.replace(/"/g, '&quot;').replace(/'/g, '&#39;');
	},

	/* 毫秒年龄 → 「3 秒前」/「2 分钟前」 */
	ago: function (ms) {
		if (ms == null) return this.T('Never');
		var s = Math.max(1, Math.round(ms / 1000));
		if (s < 90) return s + 's';
		if (s < 5400) return Math.round(s / 60) + 'min';
		return Math.round(s / 3600) + 'h';
	},

	/* 字节 → 可读 */
	bytes: function (n) {
		if (n == null || isNaN(n)) return '-';
		var u = ['B', 'KB', 'MB', 'GB', 'TB'], i = 0, v = Number(n);
		while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
		return (i === 0 ? v : v.toFixed(v >= 100 ? 0 : 1)) + ' ' + u[i];
	},

	/* 速率（B/s）→ 可读 */
	rate: function (bps) {
		if (bps == null || isNaN(bps) || bps < 0) return '-';
		return this.bytes(bps) + '/s';
	},

	/* CSQ RSSI（dBm）→ 0~5 格 */
	bars: function (rssi) {
		if (rssi == null || isNaN(rssi) || rssi === 0) return 0;
		if (rssi >= -65) return 5;
		if (rssi >= -75) return 4;
		if (rssi >= -85) return 3;
		if (rssi >= -95) return 2;
		if (rssi >= -109) return 1;
		return 0;
	},

	/* 信号等级文案（与 modemd 原生页一致的口径） */
	rssiText: function (rssi) {
		if (rssi == null || isNaN(rssi) || rssi === 0) return '-';
		return rssi + ' dBm';
	},

	/* 注册状态行 */
	regText: function (self, reg) {
		if (!reg) return '-';
		if (reg.roaming) return this.T('Roaming');
		return reg.registered ? this.T('Registered') : this.T('Not registered');
	},

	/* 表格单元的兜底文案 */
	dash: function (v) {
		return (v == null || v === '' || v === '-') ? '-' : String(v);
	},

	/* 时间戳（ISO）→ 页面短时间 */
	time: function (iso) {
		if (!iso) return '-';
		var d = new Date(iso);
		if (isNaN(d.getTime())) return iso;
		return ('0' + d.getHours()).slice(-2) + ':' + ('0' + d.getMinutes()).slice(-2) + ':' + ('0' + d.getSeconds()).slice(-2);
	}
});
