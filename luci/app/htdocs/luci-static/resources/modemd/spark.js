'use strict';
'require baseclass';

/* 实时速率曲线（轻量 SVG，无第三方图表库）
 *
 * 数据源：`GET /api/traffic`（`runtime/ifstat.rs` 后台 1000ms 采样，
 * 与页面是否打开无关）。这里**只负责画**，不做任何统计 —— 速率、累计、
 * 历史点全部由后端算好，避免 LuCI 与内置 WebUI 各算一套、数字对不上。
 *
 * 为什么自己画而不是引 ECharts/Chart.js：
 *   · 固件上的 LuCI 不该为一条双线图多拉几百 KB；
 *   · 需求很窄（60 个点、两条线、纵轴自适应），SVG 三个 polyline 就够。
 *
 * 坐标约定：横轴固定 60 个采样点（等距，1 秒一格）；纵轴按**本窗口峰值**自适应，
 * 避免「100 Mbps 时曲线贴底」和「1 kbps 时曲线贴顶」两种极端。
 */

/* 纵轴「好看的上界」：把 max 向上取整到 1 / 2 / 5 × 10^n。
 * 例：1.3 Mbps → 2 Mbps；7.4 Mbps → 10 Mbps；0 → 1 kbps（避免除零，也给条基线）。 */
function niceCeil(v) {
	if (!(v > 0)) return 1000;
	var exp = Math.floor(Math.log(v) / Math.LN10);
	var base = Math.pow(10, exp);
	var f = v / base;
	var nice = f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10;
	return nice * base;
}

/* 生成曲线 SVG 字符串。
 *   points: [{t_ms, up_bps, down_bps}]
 *   opts:   {w, h, upColor, downColor, id} —— 宽高与配色可覆盖
 */
function render(points, opts) {
	opts = opts || {};
	var w = opts.w || 1000;
	var h = opts.h || 180;
	/* 左边留白给纵轴刻度，底部留白给时间标签 */
	var padL = 62, padR = 12, padT = 14, padB = 22;
	var iw = Math.max(10, w - padL - padR);
	var ih = Math.max(10, h - padT - padB);
	var upColor = opts.upColor || '#f59e0b';     /* 上行：琥珀 */
	var downColor = opts.downColor || '#3b82f6'; /* 下行：蓝 */
	var uid = opts.id || 'spk' + Math.floor(Math.random() * 1e9);

	var pts = Array.isArray(points) ? points : [];
	/* 只取最近 60 个（后端已裁剪，这里再兜一层：老固件/旧缓存可能给多了） */
	if (pts.length > 60) pts = pts.slice(pts.length - 60);

	var maxV = 0;
	for (var i = 0; i < pts.length; i++) {
		if (pts[i].up_bps > maxV) maxV = pts[i].up_bps;
		if (pts[i].down_bps > maxV) maxV = pts[i].down_bps;
	}
	var top = niceCeil(maxV);

	/* 横轴按**固定 60 格**排：点少时靠右对齐（新数据在右），
	   这样刚打开页面时曲线从右边长出来，而不是被拉伸铺满。 */
	var slots = 60;
	var offset = slots - pts.length;
	function xOf(idx) {
		return padL + (iw * (offset + idx)) / Math.max(1, slots - 1);
	}
	function yOf(bps) {
		/* 裁剪到 [0, top]：异常大的单点（计数器跳变）不会把整条线压扁 */
		var v = Math.min(Math.max(bps, 0), top);
		return padT + ih - (ih * v) / top;
	}

	function poly(key) {
		var d = [];
		for (var k = 0; k < pts.length; k++) {
			d.push((k ? 'L' : 'M') + xOf(k).toFixed(1) + ',' + yOf(pts[k][key]).toFixed(1));
		}
		return d.join(' ');
	}
	/* 面积填充：折线两端垂直落到底边闭合 */
	function area(key) {
		if (!pts.length) return '';
		var d = poly(key);
		d += ' L' + xOf(pts.length - 1).toFixed(1) + ',' + (padT + ih).toFixed(1);
		d += ' L' + xOf(0).toFixed(1) + ',' + (padT + ih).toFixed(1) + ' Z';
		return d;
	}

	/* 横向网格：4 等分，标注对应速率 */
	var grid = '';
	for (var g = 0; g <= 4; g++) {
		var gy = padT + (ih * g) / 4;
		var val = top * (4 - g) / 4;
		grid += '<line x1="' + padL + '" y1="' + gy.toFixed(1) + '" x2="' + (padL + iw) +
			'" y2="' + gy.toFixed(1) + '" stroke="currentColor" stroke-opacity="0.10" stroke-width="1"/>';
		grid += '<text x="' + (padL - 8) + '" y="' + (gy + 3.5).toFixed(1) +
			'" text-anchor="end" font-size="10" fill="currentColor" fill-opacity="0.55">' +
			shortRate(val) + '</text>';
	}

	/* 底部时间刻度：左「60s 前」、中「30s」、右「现在」 */
	var timeLbl = '';
	[[0, '60s 前'], [0.5, '30s'], [1, '现在']].forEach(function (p) {
		var tx = padL + iw * p[0];
		timeLbl += '<text x="' + tx.toFixed(1) + '" y="' + (h - 6) +
			'" text-anchor="' + (p[0] === 0 ? 'start' : p[0] === 1 ? 'end' : 'middle') +
			'" font-size="10" fill="currentColor" fill-opacity="0.45">' + p[1] + '</text>';
	});

	var empty = pts.length
		? ''
		: '<text x="' + (padL + iw / 2) + '" y="' + (padT + ih / 2) +
		  '" text-anchor="middle" font-size="12" fill="currentColor" fill-opacity="0.45">' +
		  '等待采样…（后台每秒一个点，约 2 秒后出现曲线）</text>';

	return '' +
		'<svg viewBox="0 0 ' + w + ' ' + h + '" width="100%" height="' + h +
		'" preserveAspectRatio="none" style="display:block;color:var(--mp-neutral-700,#374151)">' +
		'<defs>' +
			'<linearGradient id="' + uid + '-up" x1="0" y1="0" x2="0" y2="1">' +
				'<stop offset="0%" stop-color="' + upColor + '" stop-opacity="0.28"/>' +
				'<stop offset="100%" stop-color="' + upColor + '" stop-opacity="0"/>' +
			'</linearGradient>' +
			'<linearGradient id="' + uid + '-dn" x1="0" y1="0" x2="0" y2="1">' +
				'<stop offset="0%" stop-color="' + downColor + '" stop-opacity="0.28"/>' +
				'<stop offset="100%" stop-color="' + downColor + '" stop-opacity="0"/>' +
			'</linearGradient>' +
		'</defs>' +
		grid +
		(pts.length ? '<path d="' + area('down_bps') + '" fill="url(#' + uid + '-dn)"/>' : '') +
		(pts.length ? '<path d="' + area('up_bps') + '" fill="url(#' + uid + '-up)"/>' : '') +
		(pts.length ? '<path d="' + poly('down_bps') + '" fill="none" stroke="' + downColor +
			'" stroke-width="1.8" stroke-linejoin="round" stroke-linecap="round"/>' : '') +
		(pts.length ? '<path d="' + poly('up_bps') + '" fill="none" stroke="' + upColor +
			'" stroke-width="1.8" stroke-linejoin="round" stroke-linecap="round"/>' : '') +
		/* 最新一点画个圆点，方便一眼看到「现在」 */
		(pts.length ? '<circle cx="' + xOf(pts.length - 1).toFixed(1) + '" cy="' +
			yOf(pts[pts.length - 1].down_bps).toFixed(1) + '" r="2.6" fill="' + downColor + '"/>' : '') +
		(pts.length ? '<circle cx="' + xOf(pts.length - 1).toFixed(1) + '" cy="' +
			yOf(pts[pts.length - 1].up_bps).toFixed(1) + '" r="2.6" fill="' + upColor + '"/>' : '') +
		empty +
		timeLbl +
		'</svg>';
}

/* 纵轴刻度的紧凑写法（1.5M / 800K / 20K） */
function shortRate(v) {
	if (!(v > 0)) return '0';
	if (v >= 1e9) return (v / 1e9).toFixed(v >= 1e10 ? 0 : 1) + 'G';
	if (v >= 1e6) return (v / 1e6).toFixed(v >= 1e7 ? 0 : 1) + 'M';
	if (v >= 1e3) return (v / 1e3).toFixed(v >= 1e4 ? 0 : 1) + 'K';
	return String(Math.round(v));
}

return baseclass.extend({
	__name__: 'modemd.spark',
	render: render,
	niceCeil: niceCeil,
	shortRate: shortRate
});
