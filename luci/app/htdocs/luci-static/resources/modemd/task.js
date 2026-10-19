'use strict';
'require baseclass';

/* modemd 异步任务传输层（LuCI 侧）：提交 → 轮询 → 取结果。
 *
 * 后端契约见 rust/src/http/tasks.rs：
 *   POST /api/tasks     {kind, modem_id, ...}  → {task:{id, state:'queued'|'running',...}}
 *   GET  /api/tasks/get?id=tN                    → {task:{state, result, error, progress, elapsed_ms}}
 *   POST /api/tasks/cancel {id}                  → {cancelled}
 *
 * 状态机：queued → running → done | failed | cancelled。
 */

var DEFAULT_POLL_MS = 600;   /* 首轮轮询间隔 */
var MAX_POLL_MS = 2000;      /* 退避上限 */
var DEFAULT_TIMEOUT = 180000;

return baseclass.extend({
	__name__: 'modemd.task',

	/* 跑一个任务并返回结果（data 部分，与同步接口的 data 同形）。
	   opts.onTick(taskView) 在每次轮询后调用；opts.signal.abort 可外部取消。 */
	run: function (api, kind, params, opts) {
		opts = opts || {};
		var self = this;
		var interval = opts.interval || DEFAULT_POLL_MS;
		var deadline = Date.now() + (opts.timeout || DEFAULT_TIMEOUT);
		var stateObj = { cancelled: false, currentId: null };

		function abort() {
			stateObj.cancelled = true;
			if (stateObj.currentId) {
				api.taskCancel(stateObj.currentId).catch(function () {});
			}
		}

		if (opts.signal && opts.signal.addEventListener)
			opts.signal.addEventListener('abort', abort);

		var promise = api.taskSubmit(kind, params).then(function (t) {
			stateObj.currentId = t.id;
			return self._poll(api, t.id, interval, deadline, opts.onTick, stateObj);
		}).then(function (t) {
			if (t.state === 'done') return t.result;
			if (t.state === 'cancelled') throw new Error('任务已取消');
			throw new Error(t.error || ('任务失败（' + t.state + '）'));
		});

		promise.cancel = abort;
		promise.abort = abort;
		return promise;
	},

	/* 轮询直到终态；期间做指数退避（600ms → 2000ms），失败瞬间不刷爆代理 */
	_poll: function (api, id, interval, deadline, onTick, stateObj) {
		var self = this;
		var cur = interval;
		function once() {
			if (stateObj && stateObj.cancelled) throw new Error('任务已取消');
			if (Date.now() > deadline) throw new Error('等待超时（任务 ' + id + ' 仍在后台，可到任务列表查看）');
			return api.taskGet(id).then(function (t) {
				if (typeof onTick === 'function') {
					try { onTick(t); } catch (e) { /* 回调出错不影响轮询 */ }
				}
				if (!t.state || t.state === 'queued' || t.state === 'running') {
					cur = Math.min(cur * 1.3, MAX_POLL_MS);
					return new Promise(function (resolve) {
						window.setTimeout(function () {
							resolve(self._poll(api, id, cur, deadline, onTick, stateObj));
						}, cur);
					});
				}
				return t;
			});
		}
		return once();
	}
});
