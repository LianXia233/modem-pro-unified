// LuCI 侧只读端点：把**当前已登录会话的 ID** 交给嵌在 LuCI 页面里的小窗。
//
// 为什么需要它（真机实测的约束）：
//   · LuCI 自己的会话 cookie 是 `sysauth_http`，带 **path=/cgi-bin/luci/**、SameSite=strict、
//     HttpOnly ⇒ 浏览器**不会**把它带到 modemd 的端口（:9527），JS 也读不到；
//   · 而 modemd 需要“你确实登录过路由器”这个事实才能放行（会话有效性交给 ubus
//     session.get 判定，见 modemd 的 ubus/session.rs 与 http/auth.rs）。
// 所以让 LuCI 这边（本端点）把会话 ID 交给页面，页面再拼参/写 cookie 传给 modemd。
// 未登录时 dispatcher 在控制器之前就回 403（带 X-LuCI-Login-Required: yes 响应头，
// 与 admin/uci/* 同一套机制），调用方按"没登录"处理即可；modemd 会退回普通
// 登录模式，功能不受影响。
//
// 端点：GET /cgi-bin/luci/admin/modemd_auth/session  →  {"sid":"<32位会话ID>"} 或 {"sid":null}
// 安全性：GET 不带 CSRF token 也安全 —— 跨站请求带不上 SameSite=strict 的 cookie，
//         响应也没有 CORS 头（跨源读不到）；返回内容只有会话 ID，没有任何别的信息。
//         节点按 menu.d 的范式声明 auth.methods（cookie:sysauth_http/https），
//         这样 dispatcher 才会去解析会话；不带 cors（跨源读不到是刻意的）。
// 分发方式：本文件只提供 action_session()，节点注册在 menu.d/luci-app-modemd.json
//           （ucode 控制器不像 Lua 控制器那样被 dispatcher 自动扫到，必须显式注册
//           type:function 节点，否则这个端点根本访问不到）。
// 语言坑：ucode **没有 typeof 运算符**，判断类型只能用 type() 函数
//         （写 typeof 会在 require 时报 Syntax error，整个 500）。
'use strict';

return {
	action_session: function() {
		http.prepare_content('application/json; charset=UTF-8');
		// ctx 由 dispatcher 挂上（resolve_page 里 ctx.authsession = session?.sid）；
		// 匿名/无会话时 authsession 是 null，这里如实返回 null。
		let sid = null;
		if (type(ctx) === 'object' && ctx && ctx.authsession)
			sid = ctx.authsession;
		http.write_json({ sid: sid });
	}
};
