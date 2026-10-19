'use strict';
'require view';
'require ui';
'require poll';

/* 5G 终端主页面：页内 Tab 菜单（设备中心 / 拨号 / 基站 / 锁网 / 短信 / 工具 / 模块 / 服务）。
 *
 * 为什么不把 8 个页面继续挂在菜单树里：
 *   设备固件（ImmortalWrt SNAPSHOT）的 ucode dispatcher 只有 firstchild 一种子菜单渲染，
 *   8 个子页面会全部平铺进侧栏；改成 Tab 后菜单只有一个入口，页内切换。
 *
 * 实现方式：本视图只做 Tab 骨架，各功能页仍是独立的 view 模块
 * （view/modemd/*.js），按需 `L.require` 动态加载并 render ——
 * 切 Tab 才加载对应 JS 与数据，与异步化口径一致（不用的页不发请求）。
 *
 * Tab 状态记在 location.hash（#md=dial），刷新/分享链接都能落在同一个 Tab。
 */

var TABS = [
	[ 'overview', '设备中心' ],
	[ 'dial', '拨号上网' ],
	[ 'cells', '基站扫描' ],
	[ 'lock', '锁网' ],
	[ 'sms', '短信' ],
	[ 'tools', '调试工具' ],
	[ 'modules', '支持模块' ],
	[ 'service', '服务控制' ]
];

function currentTab() {
	var m = /[#&]md=([a-z]+)/.exec(window.location.hash || '');
	if (m) {
		for (var i = 0; i < TABS.length; i++)
			if (TABS[i][0] === m[1]) return m[1];
	}
	return TABS[0][0];
}

return view.extend({
	load: function () {
		return Promise.resolve();
	},

	render: function () {
		var self = this;

		var bar = E('div', {
			'id': 'md-tabbar',
			'style': 'display:flex;flex-wrap:wrap;gap:4px;border-bottom:2px solid #ccc;margin:10px 0 14px;padding:0'
		}, []);

		var body = E('div', { 'id': 'md-body' }, []);

		var root = E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '5G 终端'),
			E('div', { 'style': 'font-size:12px;color:#888' },
				'由 modemd（modem-pro）提供；Tab 内数据按需加载，慢操作走后台任务。'),
			bar,
			body
		]);

		window.addEventListener('hashchange', function () { self.show(body, bar); });
		this.show(body, bar);

		return root;
	},

	show: function (body, bar) {
		var self = this;
		var name = currentTab();

		/* Tab 按钮 */
		bar.innerHTML = '';
		TABS.forEach(function (t) {
			var active = (t[0] === name);
			bar.appendChild(E('a', {
				'href': '#md=' + t[0],
				'style': 'padding:7px 16px;text-decoration:none;border-radius:6px 6px 0 0;' +
					(active
						? 'background:#fff;border:2px solid #ccc;border-bottom-color:#fff;margin-bottom:-2px;font-weight:bold'
						: 'background:transparent;border:2px solid transparent;color:#555'),
				'onclick': function () { /* 让 hashchange 驱动，别在此处双重渲染 */ }
			}, t[1]));
		});

		/* 卸掉上一个 Tab 的订阅/轮询残留（各视图用 store.on 注册，切走即失效） */
		try { store_reset(); } catch (e) {}

		body.innerHTML = '';
		var loading = E('p', { 'class': 'muted', 'style': 'padding:30px 0' }, '加载中…');
		body.appendChild(loading);

		/* 动态加载对应视图并渲染（等价于 LuCI 自己的 loadView 流程） */
		L.resolveDefault(L.require('view.modemd.' + name)).then(function (Cls) {
			if (!Cls)
				throw new Error('视图不存在：view/modemd/' + name + '.js');
			var inst = new Cls();
			return Promise.resolve(inst.load ? inst.load() : null)
				.then(function (data) { return inst.render(data); });
		}).then(function (dom) {
			body.innerHTML = '';
			body.appendChild(dom);
		}).catch(function (e) {
			body.innerHTML = '';
			body.appendChild(E('div', { 'class': 'alert-message error' }, [
				E('p', {}, '加载「' + name + '」失败：' + (e.message || e))
			]));
		});
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});

/* store 的单例引用延迟解析（避免 require 顺序问题）；main 自身不订阅轮询 */
function store_reset() {
	if (!store_reset._mod) {
		store_reset._mod = L.resolveDefault(L.require('modemd.store'));
	}
	return store_reset._mod.then(function (store) {
		if (store && store.reset) store.reset();
	});
}
