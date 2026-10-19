// modemd 的 LuCI 侧同源代理：把 /api/* 转发给本机 modemd。
//
// ## 为什么需要它（而不是让页面直接 fetch :9527）
//
//   · 浏览器跨端口是跨源，fetch 需要 CORS + 凭据，modemd 没有（也不该）为此开洞；
//   · modemd 默认要求「登录过路由器」（require_luci_login），而 LuCI 的会话 cookie
//     带 path=/cgi-bin/luci/，浏览器不会带到别的端口；
//   · 走本机 127.0.0.1 请求时 modemd 直接放行（它自己的 is_authorized 对 loopback 免登录），
//     鉴权只剩 LuCI 这一道 —— 这正是不再用 iframe/独立 WebUI 之后该有的口径。
//
// ## 异步化口径（2026-10-19）
//
// 本代理只做**快操作**（毫秒~秒级）：
//   · 读：`/api/snapshot`、`/api/modems`、`/api/dial`、`/api/sms/read`、`/api/tasks/get` …
//   · 提交：`POST /api/tasks`（立即返回 task_id）、`POST /api/tasks/cancel`
//   · 其余快写：开关类 POST（拨号开关、定时任务…）
//
// **慢操作一律走任务**：扫小区、扫网、AT、USSD、锁频段、切 USB 模式、发短信，
// 由后端 `http/tasks.rs` 在工作线程里跑，页面轮询 `/api/tasks/get`。
// 因此这里把超时收紧到 20s：超过它说明「有人把慢接口又走成了同步」，应当修前端。
//
// ## 并发模型
//
// uhttpd 每个请求起一个 ucode 进程，所以「页面并发 6 个请求」= 6 个独立进程，
// 互不阻塞；单个请求内的 `fs.popen` 是同步的，但按上面的口径它最多占 20s
// （正常都在 0.2s 内）。真正的长等待发生在浏览器侧的轮询里，不占 LuCI 任何资源。
//
// ## 契约（前端 modemd.api 模块唯一使用者）
//
//   GET/POST /cgi-bin/luci/admin/modemd_api/proxy
//     form: token(可选), path=/api/..., method=GET|POST, body=<JSON 串>
//   返回：后端原始 JSON 信封 `{code, msg, data}`（不解包、不改写）。
//
// 语言坑：ucode **没有 typeof 运算符**，只能用 type()（写 typeof 直接 Syntax error）。
'use strict';

let fs = require('fs');

const BASE = 'http://127.0.0.1:9527';
const MAX_BODY = 200 * 1024;
const TIMEOUT = 20;

// 只代理 /api/ 下的路径；正则同时挡掉空格、引号、换行与 .. 之类的注入。
const PATH_RE = /^\/api\/[A-Za-z0-9_\/.,?=&%:+~-]*$/;

// modemd 的端口从 config.toml 读（页面「服务控制」可改）。解析失败回落 9527。
function backend_port() {
	try {
		let text = fs.readfile('/etc/modemd/config.toml');
		if (type(text) == 'string') {
			let m = match(text, /listen\s*=\s*["']?[^"'\n]*:(\d+)/);
			if (m)
				return +m[1];
		}
	}
	catch (e) { /* 文件不存在/不可读：用默认端口 */ }
	return 9527;
}

function shell_quote(s) {
	return "'" + replace(s, "'", "'\\''") + "'";
}

function fail(status, msg) {
	http.status(status, status == 400 ? 'Bad Request' : 'Bad Gateway');
	http.prepare_content('application/json; charset=UTF-8');
	http.write_json({ code: -1, msg: msg });
}

return {
	action_proxy: function() {
		let path = http.formvalue('path');
		if (type(path) != 'string' || !match(path, PATH_RE))
			return fail(400, 'path 必须是以 /api/ 开头的接口路径');

		// method 只认 GET / POST：别的动词（DELETE/PUT）modemd 也不用。
		let method = http.formvalue('method');
		if (type(method) != 'string' || method == '')
			method = http.getenv('REQUEST_METHOD') ?? 'GET';
		method = uc(method);

		let base = 'http://127.0.0.1:%d'.format(backend_port());
		let cmd;
		if (method == 'POST') {
			let body = http.formvalue('body');
			if (type(body) != 'string')
				body = '{}';
			if (length(body) > MAX_BODY)
				return fail(400, '请求体过大');

			cmd = 'wget -qO- -T %d --post-data=%s --header=%s %s'.format(
				TIMEOUT, shell_quote(body), shell_quote('Content-Type: application/json'),
				shell_quote(base + path));
		}
		else {
			cmd = 'wget -qO- -T %d %s'.format(TIMEOUT, shell_quote(base + path));
		}

		let p = fs.popen(cmd, 'r');
		if (!p)
			return fail(502, '无法执行 wget（设备上缺少 busybox wget？）');

		let out = p.read('all');
		let rc = p.close();

		http.prepare_content('application/json; charset=UTF-8');

		if (type(out) != 'string' || length(out) == 0) {
			// 常见原因：modemd 没起 / 正在重启 / 接口本身 500。把 rc 带回去便于排查。
			http.write_json({ code: -1, msg: 'modemd 无响应（wget rc=%d）'.format(rc) });
			return;
		}

		http.write(out);
	}
};
