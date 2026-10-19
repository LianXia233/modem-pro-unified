'use strict';
'require baseclass';

/* modemd 异步传输层（LuCI 侧）。
 *
 * 所有请求都走 LuCI 的同源代理（admin/modemd_api/proxy，见 ucode/controller/modemd_api.uc），
 * 浏览器只和 LuCI 会话打交道；modemd 对 loopback 免登录，鉴权只剩 LuCI 这一道。
 *
 * 异步化口径：
 *   · 快读/快写：直接 request()，超时 20s（代理侧同步等待，页面用 Promise 不卡 UI）；
 *   · 慢操作：request('/api/tasks', {...}) 提交后立即返回，用 modemd/task.js 轮询；
 *   · 任何响应都是 `{code, msg, data}`：code!=0 抛 ApiError（带 msg），data 正常返回。
 */

var PROXY = '/cgi-bin/luci/admin/modemd_api/proxy';

/* LuCI 会话的 CSRF token：POST 到 dispatcher 需要（luci.env 在页面上可用） */
function csrfToken() {
	try {
		return (typeof L !== 'undefined' && L.env && L.env.token) || '';
	}
	catch (e) { return ''; }
}

var Api = baseclass.extend({
	__name__: 'modemd.api',

	/* 快请求默认超时：比代理侧 20s 略短，保证报错发生在前端而不是静默挂起 */
	timeoutMs: 20000,

	/* 拼代理 URL */
	_url: function () {
		return PROXY;
	},

	/* 底层请求：path 必须以 /api/ 开头；body 为 JS 对象（POST 时 JSON 序列化） */
	request: function (path, opts) {
		opts = opts || {};
		var method = (opts.method || (opts.body != null ? 'POST' : 'GET')).toUpperCase();
		var self = this;

		var body = new URLSearchParams();
		body.set('path', path);
		body.set('method', method);
		body.set('token', csrfToken());
		if (method === 'POST')
			body.set('body', JSON.stringify(opts.body != null ? opts.body : {}));

		var ctrl = typeof AbortController === 'function' ? new AbortController() : null;
		var timer = ctrl ? window.setTimeout(function () { ctrl.abort(); }, self.timeoutMs) : null;

		return fetch(self._url(), {
			method: 'POST',
			headers: { 'Content-Type': 'application/x-www-form-urlencoded; charset=UTF-8' },
			body: body.toString(),
			signal: ctrl ? ctrl.signal : undefined,
			credentials: 'same-origin'
		}).then(function (res) {
			if (timer) window.clearTimeout(timer);
			/* 先取文本再解析：代理/ dispatcher 在会话过期（403 登录页）、
			   端点缺失（404）或 ucode 异常（error500）时返回的是 **HTML**，
			   直接 res.json() 会得到 "Unexpected token '<'" 这种没法排查的报错。 */
			return res.text().then(function (text) {
				if (res.status === 403)
					throw new ApiError(-403, 'LuCI 会话过期或无权限：请刷新页面重新登录');
				if (res.status === 404)
					throw new ApiError(-404, '代理端点不存在（menu.d 未生效？重装插件或重启 uhttpd）');
				var ct = String(res.headers.get('content-type') || '');
				if (res.status >= 400 && !/json/i.test(ct))
					throw new ApiError(-1, 'LuCI 代理异常（HTTP ' + res.status + '）：请重新登录后再试');
				if (!/json/i.test(ct))
					throw new ApiError(-1, '代理返回了 HTML 而不是 JSON（HTTP ' + res.status + '）：通常是 LuCI 会话过期');
				var json;
				try {
					json = JSON.parse(text);
				}
				catch (e) {
					throw new ApiError(-1, '代理响应不是合法 JSON（HTTP ' + res.status + '）');
				}
				if (!json || typeof json !== 'object')
					throw new ApiError(-1, '代理返回了非 JSON 内容');
				if (json.code !== 0)
					throw new ApiError(json.code, json.msg || ('code=' + json.code));
				return json.data;
			});
		}).catch(function (e) {
			if (timer) window.clearTimeout(timer);
			if (e instanceof ApiError) throw e;
			if (e && e.name === 'AbortError') {
				throw new ApiError(-2, '请求超时（' + Math.round(self.timeoutMs / 1000) + 's）');
			}
			/* fetch 网络层失败：代理可能整个没起来 */
			throw new ApiError(-3, '无法连接 LuCI 代理：' + (e && e.message || e));
		});
	},

	/* GET /api/xxx?modem_id=0&...；params 会被 urlencode 进 path */
	get: function (path, params) {
		var q = [];
		for (var k in (params || {})) {
			if (params[k] != null)
				q.push(encodeURIComponent(k) + '=' + encodeURIComponent(params[k]));
		}
		return this.request(q.length ? path + '?' + q.join('&') : path, { method: 'GET' });
	},

	/* POST /api/xxx（JSON 体） */
	post: function (path, body) {
		return this.request(path, { method: 'POST', body: body || {} });
	},

	/* 提交异步任务：返回 {task:{id,...}} 里的 task 对象（提交是毫秒级，不会等结果） */
	taskSubmit: function (kind, params) {
		var p = Object.assign({}, params || {});
		p.kind = kind;
		return this.post('/api/tasks', p).then(function (d) {
			if (!d || !d.task) throw new ApiError(-1, '任务提交失败（无 task）');
			return d.task;
		});
	},

	taskGet: function (id) {
		return this.get('/api/tasks/get', { id: id }).then(function (d) {
			return d.task;
		});
	},

	taskCancel: function (id) {
		return this.post('/api/tasks/cancel', { id: id });
	},

	/* 快照：页面首屏唯一入口（缓存优先，不发 AT） */
	snapshot: function (modemId) {
		var params = {};
		if (modemId != null) params.modem_id = modemId;
		return this.get('/api/snapshot', params);
	}
});

function ApiError(code, msg) {
	this.code = code;
	this.message = msg || '未知错误';
	this.name = 'ApiError';
}
ApiError.prototype = Object.create(Error.prototype);

/* ui.addNotification 友好的一句话 */
ApiError.prototype.toString = function () {
	return this.message;
};

return Api;
