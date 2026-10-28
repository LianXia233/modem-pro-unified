'use strict';
'require view';
'require ui';
'require modemd.fmt as fmt';

/* 5G 终端主页面：二级菜单直通单入口，页内横向左右滑动 TAB 栏
 *
 * 导航架构：
 *   - 在 LuCI 中仅挂载为「Mobile Network (移动网络)」下的二级单入口（admin/modem/modemd），
 *     绝无多层冗余或侧栏平铺下拉式菜单（无下拉子项）。
 *   - 页面顶部呈现全宽自适应白色毛玻璃滑动 TAB 栏（支持滚轮滑动、左右箭头微调、移动端手势与自动居中）。
 *   - 8 大业务页面（设备中心/拨号上网/基站扫描/锁网/短信/调试工具/支持模块/服务控制）
 *     全部按需异步加载与渲染，状态与路由双向绑定在 location.hash（如 #tab=dial）。
 */

var TABS = [
	{ id: 'overview', name: '设备中心' },
	{ id: 'dial',     name: '拨号上网' },
	{ id: 'cells',    name: '基站扫描' },
	{ id: 'lock',     name: '锁网' },
	{ id: 'sms',      name: '短信' },
	{ id: 'tools',    name: '调试工具' },
	{ id: 'modules',  name: '支持模块' },
	{ id: 'service',  name: '服务控制' }
];

function parseTabFromHash() {
	var h = window.location.hash || '';
	var m = /[#&](?:tab|md)=([a-z0-9_-]+)/i.exec(h);
	if (m) {
		var id = m[1].toLowerCase();
		for (var i = 0; i < TABS.length; i++) {
			if (TABS[i].id === id) return id;
		}
	}
	return 'overview';
}

return view.extend({
	currentTab: null,
	currentInst: null,

	load: function () {
		fmt.ensureCss();
	},

	render: function () {
		var self = this;

		fmt.inTabHost = true;

		var initialTab = parseTabFromHash();
		self.currentTab = initialTab;

		var bodyPane = E('div', {
			'id': 'modemd-view-body',
			'class': 'mp-view-body',
			'style': 'width:100%;min-height:500px;'
		}, [
			E('div', { 'class': 'mp-card', 'style': 'text-align:center;padding:48px 0;color:var(--mp-neutral-500);' }, [
				E('div', { 'class': 'mp-spinner', 'style': 'margin:0 auto 12px;' }),
				E('div', {}, '正在加载 5G 终端数据…')
			])
		]);

		var tabBarWrap = fmt.renderTabBar(initialTab, function (tabId) {
			self.switchTab(tabId, bodyPane);
		});

		var root = E('div', { 'class': 'mp-wrap mp-main-host', 'style': 'position:relative;' }, [
			tabBarWrap,
			bodyPane
		]);

		// 监听浏览器前进/后退/外部锚点
		window.addEventListener('hashchange', function () {
			var newTab = parseTabFromHash();
			if (newTab !== self.currentTab) {
				self.switchTab(newTab, bodyPane, true);
			}
		});

		// 立即加载初始视图
		window.requestAnimationFrame(function () {
			self.switchTab(initialTab, bodyPane, true);
		});

		return root;
	},

	switchTab: function (tabId, bodyPane, skipHash) {
		var self = this;
		self.currentTab = tabId;

		if (!skipHash) {
			if (window.history && window.history.replaceState) {
				window.history.replaceState(null, '', '#tab=' + tabId);
			} else {
				window.location.hash = '#tab=' + tabId;
			}
		}

		// 同步更新滑动 TAB 栏的高亮选中状态并自动居中
		var bar = document.getElementById('mp-sliding-tab-bar');
		if (bar) {
			var items = bar.querySelectorAll('.mp-tab-item');
			items.forEach(function (el) {
				if (el.getAttribute('data-tab-id') === tabId) {
					el.classList.add('active');
					try { el.scrollIntoView({ inline: 'center', block: 'nearest', behavior: 'smooth' }); } catch (e) {}
				} else {
					el.classList.remove('active');
				}
			});
		}

		// 停止前一视图的轮询或未完成操作
		try {
			L.resolveDefault(L.require('modemd.store')).then(function (st) {
				if (st && st.reset) st.reset();
			});
		} catch (e) {}

		bodyPane = bodyPane || document.getElementById('modemd-view-body');
		if (!bodyPane) return;

		bodyPane.innerHTML = '';
		bodyPane.appendChild(E('div', { 'class': 'mp-card', 'style': 'text-align:center;padding:48px 0;color:var(--mp-neutral-500);' }, [
			E('div', { 'class': 'mp-spinner', 'style': 'margin:0 auto 12px;' }),
			E('div', {}, '正在加载模块…')
		]));

		// 异步拉取视图组件并挂载
		L.resolveDefault(L.require('view.modemd.' + tabId)).then(function (Cls) {
			if (!Cls)
				throw new Error('未找到视图组件：view/modemd/' + tabId + '.js');
			var inst = new Cls();
			self.currentInst = inst;
			return Promise.resolve(inst.load ? inst.load() : null).then(function (data) {
				return inst.render(data);
			});
		}).then(function (dom) {
			bodyPane.innerHTML = '';
			if (dom) bodyPane.appendChild(dom);
		}).catch(function (err) {
			bodyPane.innerHTML = '';
			bodyPane.appendChild(E('div', { 'class': 'alert-message error' }, [
				E('h4', {}, '加载子视图失败 (' + tabId + ')'),
				E('p', {}, String(err && err.message ? err.message : err))
			]));
		});
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
