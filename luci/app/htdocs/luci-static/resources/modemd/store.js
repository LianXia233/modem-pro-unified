'use strict';
'require baseclass';

/* modemd 共享状态（LuCI 侧单例）：
 *
 *   · 快照轮询：页面只订阅，不各自发请求 —— N 个页面共用一条 /api/snapshot 轮询；
 *   · 可见性感知：标签页切到后台自动暂停（省电 + 不占串口/代理）；
 *   · 模组选择：跨页面记住选中的模组（sessionStorage）；
 *   · 任务提示：任务开始/结束广播给所有订阅者（页面据此显示进度条）。
 *
 * 用法：
 *   let store = L.resolveDefault(require('modemd.store'))  // 视图里直接 'require store'
 *   store.on(fn)      // 订阅；立即回放当前状态，之后每次更新再调
 *   store.start(api)  // 页面 render 时调一次（幂等）
 *   store.select(1)   // 切模组：立即触发一次拉取
 */

var POLL_MS = 3000;
var POLL_MS_BG = 15000;

var subs = [];
var state = {
	ready: false,
	err: null,
	snapshot: null,
	modems: [],
	modemId: null,
	lastUpdate: 0,
	ageText: {}
};

var api = null;
var timer = null;
var busy = false;

function notify() {
	for (var i = 0; i < subs.length; i++) {
		try { subs[i](state); } catch (e) { /* 订阅者异常不影响其它人 */ }
	}
}

function bgInterval() {
	return (typeof document !== 'undefined' && document.hidden) ? POLL_MS_BG : POLL_MS;
}

function pull() {
	if (busy || !api) return;
	busy = true;
	api.snapshot(state.modemId != null ? state.modemId : undefined).then(function (snap) {
		busy = false;
		state.err = null;
		state.ready = true;
		state.snapshot = snap;
		state.modems = (snap && snap.modems) || [];
		/* 保存的 id 已不在清单里（USB 重新枚举 / 模组重启后 id 会变）：跟着
		   快照给的实际 id 或第一台走，否则后续所有按 id 的请求都 404，
		   页面整页渲染失败（实测：id 0 → 1 之后 dial 页直接崩）。 */
		if (state.modemId != null && state.modems.length &&
		    !state.modems.some(function (m) { return m.id === state.modemId; })) {
			state.modemId = (snap && snap.modem_id != null && state.modems.some(function (m) { return m.id === snap.modem_id; }))
				? snap.modem_id
				: state.modems[0].id;
			try { window.sessionStorage.setItem('modemd.modemId', String(state.modemId)); } catch (e) {}
		}
		if (snap && snap.modem_id != null)
			state.modemId = snap.modem_id;
		state.lastUpdate = Date.now();
		notify();
		schedule();
	}).catch(function (e) {
		busy = false;
		state.err = e.message || String(e);
		/* 保存的 id 拉不到快照（模组重启中 / 已重新枚举）：清掉保存值，
		   立即不带 id 重试一次（后端回落到默认模组）。只清一次 ——
		   防止真故障（无模组）时陷入高频重试循环。 */
		if (state.modemId != null && /not found|未找到/i.test(state.err)) {
			state.modemId = null;
			try { window.sessionStorage.removeItem('modemd.modemId'); } catch (e2) {}
			schedule.call(null);
			window.setTimeout(pull, 0);
			return;
		}
		notify();
		schedule();
	});
}

function schedule() {
	if (timer) window.clearTimeout(timer);
	timer = window.setTimeout(pull, bgInterval());
}

function onVisibility() {
	/* 回到前台立刻拉一次；进后台把下一次轮询拉长 */
	if (!document.hidden && api) pull();
	else schedule();
}

return baseclass.extend({
	__name__: 'modemd.store',

	start: function (apiMod) {
		api = apiMod;
		try {
			var saved = window.sessionStorage.getItem('modemd.modemId');
			if (saved != null && saved !== '')
				state.modemId = parseInt(saved, 10);
		}
		catch (e) {}

		if (!this._visBound) {
			this._visBound = true;
			document.addEventListener('visibilitychange', onVisibility);
		}
		if (!timer) pull();
		return this;
	},

	on: function (fn) {
		if (subs.indexOf(fn) < 0) subs.push(fn);
		if (state.ready || state.err) {
			try { fn(state); } catch (e) {}
		}
		return this;
	},

	off: function (fn) {
		var i = subs.indexOf(fn);
		if (i >= 0) subs.splice(i, 1);
		return this;
	},

	/* 清空全部订阅（Tab 切换时由 main.js 调用：旧视图的订阅随视图一起作废，
	   否则每切一次 Tab 就多一份闭包，且会对已脱离 DOM 的节点继续 redraw） */
	reset: function () {
		subs.length = 0;
		return this;
	},

	/* 切换选中模组：写 sessionStorage（跨页面记忆）并立刻重拉 */
	select: function (modemId) {
		state.modemId = modemId;
		try { window.sessionStorage.setItem('modemd.modemId', String(modemId)); } catch (e) {}
		pull();
		return this;
	},

	/* 立即刷新一次（任务完成后页面调用） */
	refresh: function () {
		pull();
		return this;
	},

	/* 一次性拉快照（不想订阅轮询的页面用，例如支持模块页） */
	fetch: function (apiMod, modemId) {
		return apiMod.snapshot(modemId != null ? modemId : (state.modemId != null ? state.modemId : undefined));
	},

	/* 当前选中的模组 id */
	id: function () {
		return state.modemId;
	},

	/* 当前快照（可能为 null：还没拉到过） */
	snap: function () {
		return state.snapshot;
	},

	/* 订阅者列表调试用 */
	_subs: function () { return subs.length; }
});
