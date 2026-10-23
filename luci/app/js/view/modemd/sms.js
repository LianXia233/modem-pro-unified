'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';
'require modemd.icons as icons';

/* 短信与企业微信推送管理视图（白色毛玻璃架构）
 *
 * 全面按内置 WebUI 对齐：
 *   - 短信列表（存储用量、切存储区 SM/ME/MT、长短信合流、搜索过滤、单删、全清）
 *   - 发送短信（实时字数计算、UCS2/GSM7 编码检测、分段预估、异步任务分段发送）
 *   - 企业微信机器人推送（免外部库离线 SVG 二维码、扫码轮询、登录凭据写入、
 *     模板变量定制、发送测试消息、登出）
 */

/* 存储区代号 → 中文名（后端 SmsStorage 返回的是 SM / ME / MT 原文）
   空值：FM350 一类固件的 AT+CPMS? 只回六个数字、不带区名，此时如实说「未报区名」，
   不能猜成 SM —— 猜错会让用户以为短信存在另一个区里。 */
function memLabel(m) {
	if (m === 'SM') return 'SIM 卡 (SM)';
	if (m === 'ME') return '模组内置 (ME)';
	if (m === 'MT') return '混合存储 (MT)';
	return m || '未报区名';
}

return view.extend({
	load: function () {
		fmt.ensureCss();
		store.start(api);
		var id = store.id() != null ? store.id() : undefined;
		return Promise.all([
			api.get('/api/sms/read', { modem_id: id }).catch(function () { return []; }),
			api.get('/api/sms/storage', { modem_id: id }).catch(function () { return {}; }),
			api.get('/api/wechat').catch(function () { return {}; })
		]);
	},

	render: function (data) {
		var self = this;
		var list = Array.isArray(data[0]) ? data[0] : [];
		var storage = data[1] || {};
		var wechat = (data[2] && data[2].data) ? data[2].data : (data[2] || {});
		var id = store.id() != null ? store.id() : 0;

		var curTab = 'list';
		var pollTimer = null;

		/* 发送短信弹窗 */
		function openSendModal() {
			var mask = E('div', { 'class': 'mp-modal-mask' }, [
				E('div', { 'class': 'mp-modal' }, [
					E('div', { 'class': 'mp-modal-hd' }, [
						E('h4', { 'class': 'mp-modal-title' }, '发送短信'),
						E('button', {
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'click': function () { document.body.removeChild(mask); }
						}, '✕')
					]),
					E('div', { 'style': 'display:flex;flex-direction:column;gap:12px' }, [
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;color:var(--mp-neutral-700);margin-bottom:4px;font-weight:500' }, '收件人号码:'),
							E('input', {
								'id': 'modal-sms-phone',
								'class': 'mp-input',
								'style': 'width:100%',
								'placeholder': '例如 +8613800138000'
							})
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;color:var(--mp-neutral-700);margin-bottom:4px;font-weight:500' }, '短信正文:'),
							E('textarea', {
								'id': 'modal-sms-content',
								'class': 'mp-input',
								'style': 'width:100%;height:100px;resize:vertical',
								'placeholder': '输入短信正文…'
							})
						]),
						E('div', { 'id': 'modal-sms-counter', 'style': 'font-size:12px;color:var(--mp-neutral-500);display:flex;justify-content:space-between' }, [
							E('span', {}, '长度: 0 字'),
							E('span', {}, '编码: ASCII (单条限 160 字)')
						])
					]),
					E('div', { 'class': 'mp-modal-ft' }, [
						E('button', {
							'class': 'mp-btn mp-btn-ghost',
							'click': function () { document.body.removeChild(mask); }
						}, '取消'),
						E('button', {
							'id': 'modal-btn-send',
							'class': 'mp-btn mp-btn-primary',
							'click': function (b) {
								var to = (document.getElementById('modal-sms-phone').value || '').trim();
								var body = document.getElementById('modal-sms-content').value || '';
								if (!to || !body) { alert('请输入收件人号码与正文'); return; }

								b.target.disabled = true;
								b.target.textContent = '提交中…';
								task.run(api, 'sms.send', { modem_id: id, phone: to, content: body }, { timeout: 90000 })
									.then(function () {
										ui.addNotification(null, E('p', {}, '短信已发送'), 'info');
										document.body.removeChild(mask);
										window.location.reload();
									})
									.catch(function (e) {
										alert('发送失败: ' + (e.message || e));
										b.target.disabled = false;
										b.target.textContent = '发送';
									});
							}
						}, '确认发送')
					])
				])
			]);

			var textarea = mask.querySelector('#modal-sms-content');
			var counter = mask.querySelector('#modal-sms-counter');
			textarea.addEventListener('input', function () {
				var val = textarea.value;
				var len = val.length;
				var hasNonAscii = /[^\x00-\x7F]/.test(val);
				// 单条 160/70；超过一条就按 UDH 扣除后的 153/67 重算
				var seg = Math.ceil(len / (hasNonAscii ? 70 : 160)) || 1;
				if (seg > 1) seg = Math.ceil(len / (hasNonAscii ? 67 : 153)) || 1;
				counter.children[0].textContent = '长度: ' + len + ' 字 (预计 ' + seg + ' 条)';
				counter.children[1].textContent = hasNonAscii
					? '编码: UCS2 UTF-16 (单条约 70 字，长短信每段 67 字)'
					: '编码: GSM7 (单条约 160 字，长短信每段 153 字)';
			});

			document.body.appendChild(mask);
		}

		/* 根容器 */
		var root = E('div', { 'class': 'mp-wrap' }, [
			fmt.renderTabBar('sms'),

			/* 1. Header */
			E('div', { 'class': 'mp-header' }, [
				E('div', { 'style': 'display:flex;align-items:center;gap:12px;flex-wrap:wrap' }, [
					E('div', { 'class': 'mp-title-badge' }, icons.get('sms', 18)),
					E('div', {}, [
						E('h2', { 'class': 'mp-title-text' }, '5G 终端 · ' + fmt.T('SMS')),
						E('div', { 'class': 'mp-subtitle-text' },
							'模组短信接收 / 发送 / 会话分段 · 企业微信机器人推送')
					])
				]),
				E('div', { 'style': 'display:flex;align-items:center;gap:8px;flex-wrap:wrap' }, [
					E('button', {
						'class': 'mp-btn mp-btn-primary mp-btn-sm',
						'click': function () { openSendModal(); }
					}, [ icons.get('sms', 13), ' 发送短信' ]),
					E('button', {
						'class': 'mp-btn mp-btn-ghost mp-btn-sm',
						'click': ui.createHandlerFn(self, function () { window.location.reload(); })
					}, [ icons.get('refresh', 13), ' 刷新' ])
				])
			]),

			/* 2. 子页签导航 */
			E('div', { 'class': 'mp-subtabs' }, [
				E('button', { 'class': 'mp-subtab-btn active', 'data-tab': 'list', 'click': function (e) { self.switchTab('list', e.target); } }, [
					icons.get('sms', 15), ' 短信收件箱'
				]),
				E('button', { 'class': 'mp-subtab-btn', 'data-tab': 'wechat', 'click': function (e) { self.switchTab('wechat', e.target); } }, [
					icons.get('wechat', 15), ' 微信机器人推送'
				])
			]),

			/* 3. 子页面 1：短信收件箱 */
			E('div', { 'id': 'sub-sms-list', 'class': 'mp-subpane' }, [
				/* 存储区状态卡片 */
				E('div', { 'class': 'mp-card', 'style': 'padding:14px 20px' }, [
					E('div', { 'style': 'display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:12px' }, [
						E('div', { 'style': 'display:flex;align-items:center;gap:12px;font-size:13px' }, [
							E('span', { 'style': 'font-weight:600' }, '短信存储区:'),
							/* 后端 SmsStorage 的字段是 current（= mem1）与 mem1_used / mem1_total，
							   没有 cur / counts —— 之前读错导致徽章永远显示默认 SM、容量不显示 */
							E('span', { 'class': 'mp-badge mp-badge-primary' }, memLabel(storage.current || storage.mem1)),
							(storage.mem1_total > 0)
								? E('span', { 'style': 'color:var(--mp-neutral-500)' }, '容量: ' + storage.mem1_used + ' / ' + storage.mem1_total)
								: ''
						]),
						E('div', { 'style': 'display:flex;align-items:center;gap:8px' }, [
							E('label', { 'style': 'font-size:12px;color:var(--mp-neutral-500)' }, '切换存储区:'),
							E('select', { 'id': 'sel-sms-mem', 'class': 'mp-select', 'style': 'min-width:120px' },
								(function () {
									var mems = (storage.supported && storage.supported.length)
										? storage.supported : ['SM', 'ME', 'MT'];
									var cur = storage.current || storage.mem1 || '';
									var opts = mems.map(function (m) {
										return E('option', { 'value': m, 'selected': (cur === m) ? 'selected' : null }, memLabel(m));
									});
									/* AT+CPMS? 只回六个数字（没有区名）时下拉框无从选中：
									   加一条禁用占位，既如实反映「不知道当前区」，也挡住把空值发进 AT+CPMS */
									if (!cur) {
										opts.unshift(E('option', { 'value': '', 'selected': 'selected', 'disabled': 'disabled' },
											'模组未上报区名'));
									}
									return opts;
								})()
							),
							E('button', {
								'class': 'mp-btn mp-btn-ghost mp-btn-sm',
								'click': ui.createHandlerFn(self, function (btn) {
									var mem = document.getElementById('sel-sms-mem').value;
									if (!mem) {
										ui.addNotification(null, E('p', {}, '模组没上报当前存储区名，无法切换；请先读一次短信或改用 AT+CPMS 手动指定'), 'warning');
										return;
									}
									btn.disabled = true;
									/* 切存储区要下发 AT+CPMS 再回读确认，同步 POST 会撞上代理 20s 超时 */
									task.run(api, 'sms.storage.set', { modem_id: id, mem: mem }).then(function () {
										ui.addNotification(null, E('p', {}, '已切到存储区 ' + memLabel(mem)), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '切换失败: ' + (e.message || e)), 'error');
										btn.disabled = false;
									});
								})
							}, '切换')
						])
					])
				]),

				/* 列表与搜索操作卡片 */
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [
							E('span', { 'innerHTML': icons.sms(17) }),
							'收件箱列表 (' + list.length + ' 条)'
						]),
						E('div', { 'class': 'mp-card-actions' }, [
							E('input', {
								'id': 'sms-search-input',
								'class': 'mp-input',
								'style': 'width:180px',
								'placeholder': '过滤号码或内容…'
							}),
							E('button', {
								'class': 'mp-btn mp-btn-danger mp-btn-sm',
								'click': ui.createHandlerFn(self, function () {
									if (!confirm('确定要清空模组里的所有短信吗？此操作不可逆！')) return;
									return api.post('/api/sms/delete', { modem_id: id, all: true }).then(function () {
										ui.addNotification(null, E('p', {}, '所有短信已删除'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '删除失败: ' + (e.message || e)), 'error');
									});
								})
							}, '全部清空')
						])
					]),
					E('div', { 'id': 'sms-table-wrap' }, [ self.renderSmsTable(list, id) ])
				])
			]),

			/* 4. 子页面 2：企业微信推送管理 */
			E('div', { 'id': 'sub-sms-wechat', 'class': 'mp-subpane', 'style': 'display:none' }, [
				E('div', { 'class': 'mp-card' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [
							E('span', { 'innerHTML': icons.wechat(17) }),
							'企业微信机器人转发设置'
						]),
						E('span', { 'class': 'mp-badge ' + (wechat.logged_in ? 'mp-badge-success' : 'mp-badge-muted') },
							wechat.logged_in ? '已登录微信' : '未登录')
					]),

					/* 微信登录态与二维码展示区 */
					E('div', { 'style': 'margin-bottom:18px;padding-bottom:16px;border-bottom:1px solid var(--mp-border-glass-subtle)' }, [
						E('div', { 'style': 'display:flex;align-items:center;gap:18px;flex-wrap:wrap' }, [
							/* 二维码区域 */
							wechat.qr_png ? E('div', {
								'style': 'width:140px;height:140px;background:#fff;border-radius:12px;padding:8px;border:1px solid var(--mp-border-glass);display:flex;align-items:center;justify-content:center'
							}, [
								E('img', { 'src': wechat.qr_png, 'style': 'width:100%;height:100%;object-fit:contain' })
							]) : E('div', {
								'style': 'width:120px;height:120px;background:rgba(241,245,249,0.7);border-radius:12px;display:flex;align-items:center;justify-content:center;color:var(--mp-neutral-500);font-size:12px'
							}, '暂无二维码'),

							/* 扫码指引与操作 */
							E('div', { 'style': 'display:flex;flex-direction:column;gap:8px;max-width:440px' }, [
								E('div', { 'style': 'font-size:14px;font-weight:600;color:var(--mp-neutral-900)' },
									wechat.logged_in ? ('已成功登录企业微信 (Bot ID: ' + (wechat.bot_id || '-') + ')') : (wechat.qr_png ? '请使用企业微信 App 扫描二维码' : '点击下方按钮获取扫码登录二维码')),
								E('p', { 'style': 'font-size:12.5px;color:var(--mp-neutral-500);margin:0;line-height:1.5' },
									wechat.logged_in ? ('推送目标: ' + (wechat.to_user_id || wechat.effective_target || '未指定（将推送给最近发送者）')) : '二维码由设备后端原生离线生成，有效期内可直接扫码授权。'),
								E('div', { 'style': 'display:flex;gap:8px;margin-top:4px' }, [
									wechat.logged_in ? E('button', {
										'class': 'mp-btn mp-btn-danger mp-btn-sm',
										'click': ui.createHandlerFn(self, function (btn) {
											if (!confirm('确定要注销企业微信登录凭据吗？')) return;
											btn.disabled = true;
											api.post('/api/wechat/logout').then(function () {
												ui.addNotification(null, E('p', {}, '已注销微信登录'), 'info');
												window.location.reload();
											}).catch(function (e) {
												ui.addNotification(null, E('p', {}, '注销失败: ' + (e.message || e)), 'error');
												btn.disabled = false;
											});
										})
									}, '退出登录') : E('button', {
										'class': 'mp-btn mp-btn-primary mp-btn-sm',
										'click': ui.createHandlerFn(self, function (btn) {
											btn.disabled = true;
											btn.textContent = '获取中…';
											api.post('/api/wechat/login').then(function () {
												ui.addNotification(null, E('p', {}, '二维码已生成，请尽快扫码'), 'info');
												window.location.reload();
											}).catch(function (e) {
												ui.addNotification(null, E('p', {}, '获取失败: ' + (e.message || e)), 'error');
												btn.disabled = false;
											});
										})
									}, '获取登录二维码'),
									wechat.logged_in ? E('button', {
										'class': 'mp-btn mp-btn-ghost mp-btn-sm',
										'click': ui.createHandlerFn(self, function (btn) {
											btn.disabled = true;
											btn.textContent = '测试中…';
											api.post('/api/wechat/test').then(function () {
												ui.addNotification(null, E('p', {}, '测试通知已成功下发'), 'info');
											}).catch(function (e) {
												ui.addNotification(null, E('p', {}, '测试发送失败: ' + (e.message || e)), 'error');
											}).finally(function () { btn.disabled = false; btn.textContent = '发送测试消息'; });
										})
									}, '发送测试消息') : ''
								])
							])
						])
					]),

					/* 微信推送配置表单 */
					E('div', { 'style': 'max-width:600px;display:flex;flex-direction:column;gap:14px' }, [
						E('div', { 'style': 'display:flex;align-items:center;justify-content:space-between;padding:8px 0' }, [
							E('label', { 'style': 'font-size:13px;font-weight:600' }, '启用短信自动推送到微信:'),
							E('input', { 'id': 'wc-enabled', 'type': 'checkbox', 'checked': !!wechat.enabled })
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '推送目标成员 User ID (可留空自动识别):'),
							E('input', {
								'id': 'wc-to-user',
								'class': 'mp-input',
								'style': 'width:100%',
								'value': wechat.to_user_id || '',
								'placeholder': '留空则自动推送给最近发送过消息给机器人的成员'
							})
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;font-weight:600;margin-bottom:4px' }, '通知消息模板:'),
							E('textarea', {
								'id': 'wc-template',
								'class': 'mp-input',
								'style': 'width:100%;height:110px;resize:vertical',
								'placeholder': '例如：📩 新短信{parts}\n模组：{modem}\n号码：{number}\n时间：{time}\n内容：{content}'
							}, wechat.template || '')
						]),
						E('div', { 'style': 'display:flex;align-items:center;gap:6px;flex-wrap:wrap;font-size:12px;color:var(--mp-neutral-500)' }, [
							E('span', {}, '可用替换标签:'),
							E('span', { 'class': 'mp-badge mp-badge-muted', 'style': 'cursor:pointer', 'click': function () { self.insertTag('{modem}'); } }, '{modem}'),
							E('span', { 'class': 'mp-badge mp-badge-muted', 'style': 'cursor:pointer', 'click': function () { self.insertTag('{number}'); } }, '{number}'),
							E('span', { 'class': 'mp-badge mp-badge-muted', 'style': 'cursor:pointer', 'click': function () { self.insertTag('{time}'); } }, '{time}'),
							E('span', { 'class': 'mp-badge mp-badge-muted', 'style': 'cursor:pointer', 'click': function () { self.insertTag('{content}'); } }, '{content}'),
							E('span', { 'class': 'mp-badge mp-badge-muted', 'style': 'cursor:pointer', 'click': function () { self.insertTag('{parts}'); } }, '{parts}')
						]),
						E('div', { 'style': 'padding-top:6px' }, [
							E('button', {
								'class': 'mp-btn mp-btn-primary',
								'click': ui.createHandlerFn(self, function (btn) {
									var en = document.getElementById('wc-enabled').checked;
									var to = (document.getElementById('wc-to-user').value || '').trim();
									var tmpl = document.getElementById('wc-template').value;

									btn.disabled = true;
									btn.textContent = '保存中…';
									api.post('/api/wechat/set', {
										enabled: en,
										to_user_id: to,
										template: tmpl
									}).then(function () {
										ui.addNotification(null, E('p', {}, '微信推送设置已保存生效'), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '保存失败: ' + (e.message || e)), 'error');
										btn.disabled = false;
										btn.textContent = '保存配置';
									});
								})
							}, '保存配置')
						])
					])
				])
			])
		]);

		/* 搜索过滤绑定 */
		var searchIn = root.querySelector('#sms-search-input');
		if (searchIn) {
			searchIn.addEventListener('input', function () {
				var q = (searchIn.value || '').toLowerCase().trim();
				var filtered = list.filter(function (m) {
					return !q || (m.number && m.number.toLowerCase().indexOf(q) >= 0) || (m.content && m.content.toLowerCase().indexOf(q) >= 0);
				});
				var wrap = document.getElementById('sms-table-wrap');
				if (wrap) {
					wrap.innerHTML = '';
					wrap.appendChild(self.renderSmsTable(filtered, id));
				}
			});
		}

		/* 轮询微信扫码状态 */
		if (wechat.qr_png && !wechat.logged_in) {
			pollTimer = window.setInterval(function () {
				api.post('/api/wechat/verify').then(function (r) {
					if (r && r.state === 'logged_in') {
						window.clearInterval(pollTimer);
						ui.addNotification(null, E('p', {}, '企业微信已成功扫码登录'), 'info');
						window.location.reload();
					}
				}).catch(function () {});
			}, 3000);
		}

		return root;
	},

	switchTab: function (tabName, clickedBtn) {
		document.querySelectorAll('.mp-subtab-btn').forEach(function (b) { b.classList.remove('active'); });
		if (clickedBtn) clickedBtn.classList.add('active');

		document.getElementById('sub-sms-list').style.display = (tabName === 'list') ? 'block' : 'none';
		document.getElementById('sub-sms-wechat').style.display = (tabName === 'wechat') ? 'block' : 'none';
	},

	insertTag: function (tag) {
		var area = document.getElementById('wc-template');
		if (area) {
			area.value += tag;
			area.focus();
		}
	},

	renderSmsTable: function (list, id) {
		var self = this;
		var rows = (list || []).map(function (m) {
			var isUnread = (m.status === 'REC UNREAD' || m.status === '未读');
			return E('tr', {}, [
				E('td', { 'style': 'white-space:nowrap;font-size:12px;color:var(--mp-neutral-500)' }, m.timestamp || '-'),
				E('td', { 'style': 'font-family:ui-monospace,monospace;font-weight:600' }, m.number || '-'),
				E('td', { 'style': 'word-break:break-all;line-height:1.5' }, [
					m.content || '',
					m.part_total > 1 ? E('span', { 'class': 'mp-badge mp-badge-muted', 'style': 'margin-left:6px' }, m.part_seq + '/' + m.part_total) : ''
				]),
				E('td', {}, [
					E('span', { 'class': 'mp-badge ' + (isUnread ? 'mp-badge-warning' : 'mp-badge-muted') }, isUnread ? '未读' : '已读')
				]),
				E('td', { 'style': 'text-align:right' }, [
					E('button', {
						'class': 'mp-btn mp-btn-danger mp-btn-sm',
						'click': ui.createHandlerFn(self, function (btn) {
							btn.disabled = true;
							api.post('/api/sms/delete', { modem_id: id, index: m.index }).then(function () {
								ui.addNotification(null, E('p', {}, '已删除'), 'info');
								window.location.reload();
							}).catch(function (e) {
								ui.addNotification(null, E('p', {}, '删除失败: ' + (e.message || e)), 'error');
								btn.disabled = false;
							});
						})
					}, '删除')
				])
			]);
		});

		if (!rows.length) {
			rows = [
				E('tr', {}, [
					E('td', { 'colspan': 5, 'style': 'text-align:center;color:var(--mp-neutral-500);padding:24px' }, '当前存储区没有短信')
				])
			];
		}

		return E('table', { 'class': 'mp-table' }, [
			E('thead', {}, [
				E('tr', {}, [
					E('th', { 'style': 'width:150px' }, '接收时间'),
					E('th', { 'style': 'width:150px' }, '发件人号码'),
					E('th', {}, '短信内容'),
					E('th', { 'style': 'width:80px' }, '状态'),
					E('th', { 'style': 'width:70px;text-align:right' }, '操作')
				])
			]),
			E('tbody', {}, rows)
		]);
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
