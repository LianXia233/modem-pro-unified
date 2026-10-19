'use strict';
'require baseclass';

/* modemd 模组专用 SVG 图标库 (Custom SVG Icons with Dynamic Statuses)
 *
 * 特性：
 *   1. 统一 24x24 viewBox、1.8 描边、currentColor，自适应白色毛玻璃高对比界面；
 *   2. 状态变体：信号格数、网络连接、SIM 卡、模组状态提供动态图形与真实数据联动；
 *   3. 零外部依赖，纯原生内联 SVG，无运行时图片或远程字体要求。
 */

function svg(paths, size, cls, extra) {
	size = size || 18;
	cls = cls ? ' ' + cls : '';
	return '<svg class="mp-icon' + cls + '" width="' + size + '" height="' + size + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"' + (extra || '') + '>' + paths + '</svg>';
}

return baseclass.extend({
	__name__: 'modemd.icons',

	/* 辅助方法：按名称生成带包装的 DOM 节点 */
	get: function (name, size, arg) {
		size = size || 16;
		var html = '';
		switch (name) {
			case 'cpu':
			case 'chip':
			case 'modem':
				html = this.chip('running', size);
				break;
			case 'refresh':
				html = this.refresh(size);
				break;
			case 'clock':
				html = this.clock(size);
				break;
			case 'history':
			case 'pulse':
				html = this.pulse(size);
				break;
			case 'settings':
				html = this.settings(size);
				break;
			case 'tower':
				html = this.tower(size);
				break;
			case 'antenna':
				html = this.antenna(size);
				break;
			case 'signal':
				html = this.signal(arg != null ? arg : 5, size);
				break;
			case 'sim':
				html = this.sim(arg != null ? arg : 'ready', size);
				break;
			case 'dial':
				html = this.dial(size);
				break;
			case 'sms':
				html = this.sms(size);
				break;
			case 'wechat':
				html = this.wechat(size);
				break;
			case 'lock':
				html = this.lock(arg, size);
				break;
			case 'usb':
				html = this.usb(size);
				break;
			case 'pcie':
				html = this.pcie(size);
				break;
			case 'check':
				html = svg('<polyline points="20 6 9 17 4 12" stroke="#10b981"/>', size);
				break;
			case 'close':
				html = svg('<line x1="18" y1="6" x2="6" y2="18" stroke="#ef4444"/><line x1="6" y1="6" x2="18" y2="18" stroke="#ef4444"/>', size);
				break;
			case 'warn':
				html = svg('<path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" stroke="#f59e0b"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/>', size);
				break;
			default:
				if (typeof this[name] === 'function')
					html = this[name](size);
				else
					html = this.chip('running', size);
		}
		return this.toDOM(html);
	},

	/* 将 HTML/SVG 字符串安全解析为真正的 DOM 节点 */
	toDOM: function (htmlOrNode) {
		if (!htmlOrNode) return document.createTextNode('');
		if (htmlOrNode instanceof Node) return htmlOrNode;
		var div = document.createElement('div');
		div.innerHTML = String(htmlOrNode).trim();
		if (div.childNodes.length === 1) return div.firstChild;
		var span = document.createElement('span');
		span.className = 'mp-icon-wrap';
		span.style.display = 'inline-flex';
		span.style.alignItems = 'center';
		span.style.verticalAlign = 'middle';
		while (div.firstChild) span.appendChild(div.firstChild);
		return span;
	},

	/* 1. 动态信号图标 (0~5 格)
	 * bars: 0..5, color: 可选指定颜色或自动按等级着色
	 */
	signal: function (bars, size, color) {
		bars = Math.max(0, Math.min(5, parseInt(bars, 10) || 0));
		size = size || 22;
		var h = [6, 10, 14, 18, 22];
		var x = [1.5, 6, 10.5, 15, 19.5];
		var p = '';
		var activeColor = color || (bars >= 4 ? '#10b981' : (bars === 3 ? '#06b6d4' : (bars === 2 ? '#f59e0b' : (bars === 1 ? '#f97316' : '#ef4444'))));
		for (var i = 0; i < 5; i++) {
			var active = i < bars;
			var fill = active ? activeColor : 'rgba(148, 163, 184, 0.28)';
			p += '<rect x="' + x[i] + '" y="' + (23 - h[i]) + '" width="3.2" height="' + h[i] + '" rx="1.6" fill="' + fill + '" stroke="none"/>';
		}
		if (bars === 0) {
			p += '<line x1="2" y1="3" x2="22" y2="21" stroke="#ef4444" stroke-width="2.2" stroke-linecap="round"/>';
		}
		return '<svg class="mp-icon mp-icon-signal" width="' + size + '" height="' + size + '" viewBox="0 0 24 24" fill="none" style="vertical-align:middle;display:inline-block;flex-shrink:0;">' + p + '</svg>';
	},

	/* 专用 SVG 信号量程条：基于物理量真实比例渲染 */
	signalBar: function (pct, color, height) {
		pct = Math.max(0, Math.min(100, Math.round(pct || 0)));
		color = color || '#0284c7';
		height = height || 8;
		return '<svg class="mp-sig-bar-svg" width="100%" height="' + height + '" viewBox="0 0 100 ' + height + '" preserveAspectRatio="none" style="display:block;border-radius:' + (height / 2) + 'px;overflow:hidden;margin:4px 0;">' +
			'<rect width="100" height="' + height + '" rx="' + (height / 2) + '" fill="rgba(148, 163, 184, 0.22)"/>' +
			'<rect width="' + pct + '" height="' + height + '" rx="' + (height / 2) + '" fill="' + color + '"/>' +
		'</svg>';
	},

	/* 蜂窝基站与动态无线电波纹 (Tower with Radio Waves) */
	signalTowerRipples: function (bars, size) {
		bars = Math.max(0, Math.min(5, parseInt(bars, 10) || 0));
		var wave1 = bars >= 2 ? 'opacity="0.9"' : 'opacity="0.15" stroke-dasharray="2 2"';
		var wave2 = bars >= 4 ? 'opacity="0.9"' : 'opacity="0.15" stroke-dasharray="2 2"';
		var color = bars >= 4 ? '#10b981' : (bars >= 3 ? '#06b6d4' : (bars >= 2 ? '#f59e0b' : '#ef4444'));
		return svg(
			'<path d="M12 2v20M8 22l4-15 4 15M6 16h12M9 10h6"/>' +
			'<circle cx="12" cy="4" r="2.2" fill="' + color + '"/>' +
			'<path d="M8 3a5 5 0 0 1 8 0" stroke="' + color + '" ' + wave1 + '/>' +
			'<path d="M5 2a9 9 0 0 1 14 0" stroke="' + color + '" ' + wave2 + '/>',
			size
		);
	},

	/* 2. 蜂窝基站塔 (Cell Tower) */
	tower: function (size) {
		return svg(
			'<path d="M12 2v20M8 22l4-15 4 15M6 16h12M9 10h6"/>' +
			'<circle cx="12" cy="4" r="2" fill="currentColor"/>' +
			'<path d="M5 4a10 10 0 0 1 14 0M7.5 6.5a6 6 0 0 1 9 0"/>',
			size
		);
	},

	/* 3. 天线与无线电 (Antenna / RF) */
	antenna: function (size) {
		return svg(
			'<path d="M2 12h20M12 2v10M7 8l5-6 5 6M9 15a3 3 0 0 0 6 0M6 18a6 6 0 0 0 12 0"/>',
			size
		);
	},

	/* 4. 网络连接状态 (Network Connection: connected / dialing / idle / failed) */
	network: function (status, size) {
		if (status === 'connected') {
			return svg(
				'<circle cx="12" cy="12" r="9" stroke="#10b981"/><path d="M9 12l2 2 4-4" stroke="#10b981" stroke-width="2"/>',
				size
			);
		} else if (status === 'dialing' || status === 'connecting') {
			return svg(
				'<circle cx="12" cy="12" r="9" stroke="#f59e0b" stroke-dasharray="4 4"/>' +
				'<path d="M12 6v6l4 2" stroke="#f59e0b"/>',
				size, 'mp-spin'
			);
		} else if (status === 'failed' || status === 'error') {
			return svg(
				'<circle cx="12" cy="12" r="9" stroke="#ef4444"/>' +
				'<line x1="15" y1="9" x2="9" y2="15" stroke="#ef4444"/>' +
				'<line x1="9" y1="9" x2="15" y2="15" stroke="#ef4444"/>',
				size
			);
		}
		// idle / disconnected
		return svg(
			'<circle cx="12" cy="12" r="9" stroke="currentColor" opacity="0.6"/>' +
			'<line x1="4.93" y1="4.93" x2="19.07" y2="19.07" stroke="currentColor" opacity="0.6"/>',
			size
		);
	},

	/* 5. SIM 卡 (normal / absent / locked / error) */
	sim: function (state, size) {
		var st = String(state || '').toLowerCase();
		var alert = '';
		var strokeColor = 'currentColor';

		if (st.indexOf('absent') >= 0 || st.indexOf('not') >= 0 || st === '0' || st === 'none') {
			strokeColor = '#ef4444';
			alert = '<line x1="2" y1="2" x2="22" y2="22" stroke="#ef4444" stroke-width="2"/>';
		} else if (st.indexOf('pin') >= 0 || st.indexOf('puk') >= 0 || st.indexOf('lock') >= 0) {
			strokeColor = '#f59e0b';
			alert = '<rect x="9" y="11" width="6" height="5" rx="1" fill="#f59e0b" stroke="none"/><path d="M10 11V9a2 2 0 0 1 4 0v2" stroke="#f59e0b"/>';
		} else if (st.indexOf('ready') >= 0 || st.indexOf('ok') >= 0 || st === '1' || st === '正常') {
			strokeColor = '#10b981';
		}

		return svg(
			'<path d="M6 3h8l4 4v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" stroke="' + strokeColor + '"/>' +
			'<path d="M8 10h4M8 14h4M8 18h8" stroke="' + strokeColor + '" opacity="0.7"/>' + alert,
			size
		);
	},

	/* 6. 模组与芯片 (Module Chip: normal / offline / warning) */
	chip: function (status, size) {
		var c = status === 'offline' ? '#ef4444' : (status === 'running' ? '#0284c7' : 'currentColor');
		return svg(
			'<rect x="5" y="5" width="14" height="14" rx="2" stroke="' + c + '"/>' +
			'<path d="M9 1v4M15 1v4M9 19v4M15 19v4M1 9h4M1 15h4M19 9h4M19 15h4M9 9h6v6H9z" stroke="' + c + '"/>',
			size
		);
	},

	/* 7. 拨号网关与链路 (Dial Link) */
	dial: function (size) {
		return svg(
			'<circle cx="6" cy="18" r="3"/><circle cx="18" cy="6" r="3"/><circle cx="18" cy="18" r="3"/>' +
			'<path d="M8.5 16.5l7-7M9 18h6M18 9v6"/>',
			size
		);
	},

	/* 8. 实时上行速率箭头 (Upload) */
	arrowUp: function (size) {
		return svg(
			'<path d="M12 19V5M5 12l7-7 7 7" stroke="#0ea5e9"/>',
			size
		);
	},

	/* 9. 实时下行速率箭头 (Download) */
	arrowDown: function (size) {
		return svg(
			'<path d="M12 5v14M19 12l-7 7-7-7" stroke="#10b981"/>',
			size
		);
	},

	/* 10. 短信信封 (SMS Envelope) */
	sms: function (size) {
		return svg(
			'<rect x="3" y="5" width="18" height="14" rx="2"/>' +
			'<path d="M3 7l9 6 9-6"/>',
			size
		);
	},

	/* 11. 企业微信机器人 (WeChat Work Bot) */
	wechat: function (size) {
		return svg(
			'<rect x="3" y="11" width="18" height="10" rx="2"/>' +
			'<circle cx="12" cy="5" r="2"/>' +
			'<path d="M12 7v4M8 16h.01M16 16h.01M9 19h6"/>',
			size
		);
	},

	/* 12. 锁频 / 锁网 (Lock) */
	lock: function (locked, size) {
		if (locked) {
			return svg(
				'<rect x="4" y="10" width="16" height="11" rx="2" stroke="#ef4444"/>' +
				'<path d="M7 10V7a5 5 0 0 1 10 0v3" stroke="#ef4444"/>',
				size
			);
		}
		return svg(
			'<rect x="4" y="10" width="16" height="11" rx="2"/>' +
			'<path d="M7 10V7a5 5 0 0 1 9.3-2.5"/>',
			size
		);
	},

	/* 13. 飞行模式 (Airplane) */
	airplane: function (size) {
		return svg(
			'<path d="M17.8 19.2L16 11l3.5-3.5a2.1 2.1 0 0 0-3-3L13 8 4.8 6.2c-.5-.1-.9.1-1.1.5l-.3.5c-.2.5 0 1 .4 1.3L8 12l-2 3H3.5c-.3 0-.5.1-.7.3l-.3.3c-.3.4-.2.9.2 1.2L5 18l1.2 2.3c.3.4.8.5 1.2.2l.3-.3c.2-.2.3-.4.3-.7V17l3-2 3.5 4.2c.3.4.8.6 1.3.4l.5-.3c.4-.2.6-.6.5-1.1z"/>',
			size
		);
	},

	/* 14. 数据漫游 (Roaming) */
	roam: function (size) {
		return svg(
			'<circle cx="12" cy="12" r="10"/><path d="M2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>',
			size
		);
	},

	/* 15. 模组重启 / 电源 (Power / Reset) */
	reboot: function (size) {
		return svg(
			'<path d="M12 2v4M18.36 6.64a9 9 0 1 1-12.73 0"/>',
			size
		);
	},

	/* 16. 时钟同步 (Clock / NTP) */
	clock: function (size) {
		return svg(
			'<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 3"/>',
			size
		);
	},

	/* 17. 仪表盘 / 流量配额 (Gauge / Quota) */
	gauge: function (size) {
		return svg(
			'<path d="M12 15l3-4M4 18a8 8 0 1 1 16 0"/><circle cx="12" cy="15" r="1.5"/>',
			size
		);
	},

	/* 18. 定时任务 (Calendar / Timer) */
	calendar: function (size) {
		return svg(
			'<rect x="3" y="4" width="18" height="18" rx="2"/>' +
			'<line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/>',
			size
		);
	},

	/* 19. AT 指令终端 (Terminal / Console) */
	terminal: function (size) {
		return svg(
			'<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>',
			size
		);
	},

	/* 20. 刷新旋转 (Refresh / Sync) */
	refresh: function (size) {
		return svg(
			'<path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l6.73-1.19"/>',
			size
		);
	},

	/* 21. USB 接口 (USB Plug) */
	usb: function (size) {
		return svg(
			'<circle cx="12" cy="6" r="2"/><path d="M12 8v13M12 14h-4v-3M12 17h4v-2"/>' +
			'<rect x="6" y="8" width="4" height="3"/><circle cx="16" cy="13" r="1.5"/>',
			size
		);
	},

	/* 22. PCIe 插槽与硬件 (PCIe Board) */
	pcie: function (size) {
		return svg(
			'<rect x="3" y="6" width="18" height="12" rx="2"/><path d="M7 6v12M17 6v12M10 18v2M14 18v2"/>',
			size
		);
	},

	/* 23. 波动记录 (Activity / Pulse) */
	pulse: function (size) {
		return svg(
			'<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>',
			size
		);
	},

	/* 24. 日志文档 (File / Logs) */
	logs: function (size) {
		return svg(
			'<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10 9 9 9 8 9"/>',
			size
		);
	},

	/* 25. 支持模块 (Database / Layers) */
	layers: function (size) {
		return svg(
			'<polygon points="12 2 2 7 12 12 22 7 12 2"/><polyline points="2 17 12 22 22 17"/><polyline points="2 12 12 17 22 12"/>',
			size
		);
	},

	/* 26. 齿轮设置 (Settings) */
	settings: function (size) {
		return svg(
			'<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>',
			size
		);
	}
});
