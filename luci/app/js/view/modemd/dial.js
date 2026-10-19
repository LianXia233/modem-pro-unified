'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';
'require modemd.icons as icons';

/* 拨号上网管理视图（白色毛玻璃架构）
 *
 * 全面按内置 WebUI 对齐：
 *   - 拨号总开关（POST /api/dial/enable {on: bool}，写回 [net] dial_enabled）
 *   - WAN 模式选择（POST /api/dial/mode {mode}，modem_pro / dhcp / both）
 *   - IPv6 开关与取址模式（POST /api/dial/ipv6 {on: bool}，dhcpv6 / ra / static / off）
 *   - 模组总线与接口类型识别（AT/描述符/主机驱动三路判定、可信度说明、网卡提示）
 *   - 内置拨号器后端状态（native-qmi / native-mbim、控制口、MTU、网关、DNS、QMAP 绑定）
 *   - 自动组网诊断报告（周期、应用动作、跳过记录）
 *   - 手动拨号 / 断开 / 重新协商
 *   - PDP 上下文与 APN 全生命周期管理（列表、激活/停用、先断后连、删除、新增与编辑弹窗）
 */

function rowKv(k, v, extra) {
	return E('div', { 'class': 'mp-kv-item' }, [
		E('span', { 'class': 'mp-kv-label' }, k),
		E('span', { 'class': 'mp-kv-val' }, [
			v == null || v === '' ? '-' : String(v),
			extra ? E('span', { 'style': 'margin-left:6px;font-size:11.5px;color:var(--mp-neutral-500)' }, extra) : ''
		])
	]);
}

return view.extend({
	load: function () {
		fmt.ensureCss();
		store.start(api);
		var id = store.id() != null ? store.id() : undefined;
		return Promise.all([
			store.fetch(api),
			api.get('/api/dial', { modem_id: id }),
			api.get('/api/params', { modem_id: id }).catch(function () { return {}; })
		]);
	},

	render: function (data) {
		var self = this;
		var snap = data[0] || {};
		var dial = data[1] || {};
		var params = data[2] || {};
		var st = dial.state || {};
		var usb = dial.usb || {};
		var hostDialer = dial.host_dialer;
		var autoWan = dial.auto_wan;
		var id = dial.modem_id != null ? dial.modem_id : (snap.modem_id != null ? snap.modem_id : 0);

		var isConnected = st.state === 'connected';
		var isDialing = st.state === 'dialing';
		var statusText = st.state_text || st.state || '未拨号';
		var statusBadgeCls = isConnected ? 'mp-badge-success' : (isDialing ? 'mp-badge-warning' : 'mp-badge-muted');

		/* 弹窗创建函数 */
		function openApnModal(apnItem) {
			var isEdit = !!apnItem;
			var cidVal = isEdit ? apnItem.cid : 1;
			var apnVal = isEdit ? (apnItem.apn || '') : '';
			var pdpVal = isEdit ? (apnItem.pdp_type || 'IPV4V6') : 'IPV4V6';

			var mask = E('div', { 'class': 'mp-modal-mask' }, [
				E('div', { 'class': 'mp-modal' }, [
					E('div', { 'class': 'mp-modal-hd' }, [
						E('h4', { 'class': 'mp-modal-title' }, isEdit ? '编辑 APN (CID ' + cidVal + ')' : '新增 APN 上下文'),
						E('button', {
							'class': 'mp-btn mp-btn-ghost mp-btn-sm',
							'click': function () { document.body.removeChild(mask); }
						}, '✕')
					]),
					E('div', { 'style': 'display:flex;flex-direction:column;gap:12px' }, [
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;color:var(--mp-neutral-700);margin-bottom:4px;font-weight:500' }, 'CID 上下文编号 (1~16):'),
							E('input', {
								'id': 'modal-apn-cid',
								'class': 'mp-input',
								'style': 'width:100%',
								'type': 'number',
								'min': '1',
								'max': '16',
								'value': String(cidVal),
								'disabled': isEdit
							})
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;color:var(--mp-neutral-700);margin-bottom:4px;font-weight:500' }, '接入点名称 (APN):'),
							E('input', {
								'id': 'modal-apn-name',
								'class': 'mp-input',
								'style': 'width:100%',
								'placeholder': '例如 ctnet / 3gnet / cmnet',
								'value': apnVal
							})
						]),
						E('div', {}, [
							E('label', { 'style': 'display:block;font-size:12.5px;color:var(--mp-neutral-700);margin-bottom:4px;font-weight:500' }, '协议类型 (PDP Type):'),
							E('select', { 'id': 'modal-apn-pdp', 'class': 'mp-select', 'style': 'width:100%' }, [
								E('option', { 'value': 'IPV4V6', 'selected': pdpVal === 'IPV4V6' }, 'IPv4/IPv6 双栈 (IPV4V6)'),
								E('option', { 'value': 'IP', 'selected': pdpVal === 'IP' }, '仅 IPv4 (IP)'),
								E('option', { 'value': 'IPV6', 'selected': pdpVal === 'IPV6' }, '仅 IPv6 (IPV6)')
							])
						]),
						E('p', { 'style': 'font-size:12px;color:var(--mp-neutral-500);margin:0' },
							'写入后会回读确认，并自动对该 CID 执行重新激活。')
					]),
					E('div', { 'class': 'mp-modal-ft' }, [
						E('button', {
							'class': 'mp-btn mp-btn-ghost',
							'click': function () { document.body.removeChild(mask); }
						}, '取消'),
						E('button', {
							'class': 'mp-btn mp-btn-primary',
							'click': function (b) {
								var cid = parseInt(document.getElementById('modal-apn-cid').value, 10);
								var apn = (document.getElementById('modal-apn-name').value || '').trim();
								var pdp = document.getElementById('modal-apn-pdp').value;
								if (!apn) { alert('请输入 APN 名称'); return; }
								b.target.disabled = true;
								b.target.textContent = '写入中…';
								api.post('/api/params/apn', {
									modem_id: id,
									cid: cid,
									apn: apn,
									pdp_type: pdp
								}).then(function (r) {
									ui.addNotification(null, E('p', {}, (r && r.msg) || 'APN 已写入并激活'), 'info');
									document.body.removeChild(mask);
									window.location.reload();
								}).catch(function (e) {
									alert('写入失败: ' + (e.message || e));
									b.target.disabled = false;
									b.target.textContent = '保存';
								});
							}
						}, '保存并下发')
					])
				])
			]);
			document.body.appendChild(mask);
		}

		/* 组装页面 */
		var root = E('div', { 'class': 'mp-wrap' }, [
			fmt.renderTabBar('dial'),

			/* 1. 顶部状态与拨号控制条 */
			E('div', { 'class': 'mp-header' }, [
				E('div', { 'style': 'display:flex;align-items:center;gap:12px;flex-wrap:wrap' }, [
					E('div', { 'class': 'mp-title-badge' }, E('span', { 'innerHTML': icons.dial(20) })),
					E('div', {}, [
						E('h2', { 'class': 'mp-title-text' }, '5G 终端 · ' + fmt.T('Dial-up')),
						E('div', { 'class': 'mp-subtitle-text' },
							'内置 QMI / MBIM / ECM / RNDIS 数据呼叫托管 · 自动组网')
					]),
					E('span', { 'class': 'mp-badge ' + statusBadgeCls }, statusText)
				]),
				E('div', { 'style': 'display:flex;align-items:center;gap:8px;flex-wrap:wrap' }, [
					E('button', {
						'class': 'mp-btn mp-btn-primary',
						'click': ui.createHandlerFn(self, function (btn) {
							btn.disabled = true;
							var orig = btn.textContent;
							btn.textContent = '拨号中…';
							return api.post('/api/dial/connect', { modem_id: id }).then(function (d) {
								ui.addNotification(null, E('p', {}, (d && d.msg) || '已交办内置拨号器，正在建立会话'), 'info');
								store.refresh();
								window.setTimeout(function () { window.location.reload(); }, 2000);
							}).catch(function (e) {
								ui.addNotification(null, E('p', {}, '拨号失败: ' + (e.message || e)), 'error');
							}).finally(function () { btn.disabled = false; btn.textContent = orig; });
						})
					}, [ E('span', { 'innerHTML': icons.dial(14) }), ' 一键拨号' ]),
					E('button', {
						'class': 'mp-btn mp-btn-danger',
						'click': ui.createHandlerFn(self, function (btn) {
							if (!confirm('确定断开当前模组的数据连接吗？')) return;
							btn.disabled = true;
							var orig = btn.textContent;
							btn.textContent = '断开中…';
							return api.post('/api/dial/disconnect', { modem_id: id }).then(function (d) {
								ui.addNotification(null, E('p', {}, (d && d.msg) || '数据连接已断开'), 'info');
								store.refresh();
								window.setTimeout(function () { window.location.reload(); }, 1500);
							}).catch(function (e) {
								ui.addNotification(null, E('p', {}, '断开失败: ' + (e.message || e)), 'error');
							}).finally(function () { btn.disabled = false; btn.textContent = orig; });
						})
					}, '断开连接')
				])
			]),

			/* 2. 拨号开关与策略三件套 (拨号总开关 / WAN 模式 / IPv6 开关) */
			E('div', { 'class': 'mp-card' }, [
				E('div', { 'class': 'mp-card-hd' }, [
					E('h3', { 'class': 'mp-card-title' }, [
						E('span', { 'innerHTML': icons.settings(17) }),
						'拨号策略与网络开关'
					])
				]),
				E('div', { 'style': 'display:grid;grid-template-columns:repeat(auto-fit, minmax(280px, 1fr));gap:16px' }, [
					/* 拨号总开关 */
					E('div', { 'class': 'mp-metric-box', 'style': 'justify-content:center' }, [
						E('div', { 'style': 'display:flex;align-items:center;justify-content:space-between;margin-bottom:8px' }, [
							E('span', { 'style': 'font-weight:600;font-size:13.5px' }, '拨号总开关'),
							E('span', { 'class': 'mp-badge ' + (dial.dial_enabled ? 'mp-badge-success' : 'mp-badge-muted') }, dial.dial_enabled ? '已开启' : '已关闭')
						]),
						E('p', { 'style': 'font-size:12px;color:var(--mp-neutral-500);margin:0 0 10px' },
							'关掉后所有拨号方式（含手动与自动组网）均不再呼叫。'),
						E('button', {
							'class': 'mp-btn mp-btn-sm ' + (dial.dial_enabled ? 'mp-btn-danger' : 'mp-btn-primary'),
							'click': ui.createHandlerFn(self, function (btn) {
								var nextOn = !dial.dial_enabled;
								btn.disabled = true;
								/* 修复关键参数：必须发送 {on: bool} */
								api.post('/api/dial/enable', { on: nextOn }).then(function () {
									ui.addNotification(null, E('p', {}, nextOn ? '拨号已开启' : '拨号已关闭'), 'info');
									window.location.reload();
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '切换失败: ' + (e.message || e)), 'error');
									btn.disabled = false;
								});
							})
						}, dial.dial_enabled ? '关闭拨号' : '开启拨号')
					]),

					/* WAN 模式选择 */
					E('div', { 'class': 'mp-metric-box', 'style': 'justify-content:center' }, [
						E('div', { 'style': 'display:flex;align-items:center;justify-content:space-between;margin-bottom:8px' }, [
							E('span', { 'style': 'font-weight:600;font-size:13.5px' }, 'WAN 编排模式'),
							E('span', { 'class': 'mp-badge mp-badge-primary' }, dial.wan_mode_label || dial.wan_mode || 'modem_pro')
						]),
						E('p', { 'style': 'font-size:12px;color:var(--mp-neutral-500);margin:0 0 10px' },
							'配置网络接口类型（modem_pro 协议 / DHCP 客户端）。'),
						E('div', { 'style': 'display:flex;gap:6px' }, [
							E('select', { 'id': 'sel-wan-mode', 'class': 'mp-select', 'style': 'flex:1' },
								(dial.wan_modes || [
									{ mode: 'modem_pro', label: 'modem_pro (默认)' },
									{ mode: 'dhcp', label: 'DHCP 客户端' },
									{ mode: 'both', label: '双协议兼容' }
								]).map(function (mItem) {
									return E('option', { 'value': mItem.mode, 'selected': mItem.mode === dial.wan_mode }, mItem.label);
								})
							),
							E('button', {
								'class': 'mp-btn mp-btn-primary mp-btn-sm',
								'click': ui.createHandlerFn(self, function (b) {
									var selMode = document.getElementById('sel-wan-mode').value;
									b.disabled = true;
									api.post('/api/dial/mode', { mode: selMode }).then(function (r) {
										ui.addNotification(null, E('p', {}, 'WAN 模式已更新: ' + (r.wan_mode_label || selMode)), 'info');
										window.location.reload();
									}).catch(function (e) {
										ui.addNotification(null, E('p', {}, '切换失败: ' + (e.message || e)), 'error');
										b.disabled = false;
									});
								})
							}, '切换')
						])
					]),

					/* IPv6 开关与模式 */
					E('div', { 'class': 'mp-metric-box', 'style': 'justify-content:center' }, [
						E('div', { 'style': 'display:flex;align-items:center;justify-content:space-between;margin-bottom:8px' }, [
							E('span', { 'style': 'font-weight:600;font-size:13.5px' }, 'IPv6 支持'),
							E('span', { 'class': 'mp-badge ' + (dial.wan_ipv6 ? 'mp-badge-success' : 'mp-badge-muted') }, dial.wan_ipv6 ? '已开启' : '已关闭')
						]),
						E('p', { 'style': 'font-size:12px;color:var(--mp-neutral-500);margin:0 0 10px' },
							'模组侧 IPv6 取址方式: ' + (dial.wan_ipv6_mode || 'dhcpv6') + '。'),
						E('button', {
							'class': 'mp-btn mp-btn-sm ' + (dial.wan_ipv6 ? 'mp-btn-danger' : 'mp-btn-primary'),
							'click': ui.createHandlerFn(self, function (btn) {
								var nextOn = !dial.wan_ipv6;
								btn.disabled = true;
								/* 修复关键参数：必须发送 {on: bool} */
								api.post('/api/dial/ipv6', { on: nextOn }).then(function () {
									ui.addNotification(null, E('p', {}, nextOn ? '已开启 IPv6' : '已关闭 IPv6'), 'info');
									window.location.reload();
								}).catch(function (e) {
									ui.addNotification(null, E('p', {}, '切换失败: ' + (e.message || e)), 'error');
									btn.disabled = false;
								});
							})
						}, dial.wan_ipv6 ? '关闭 IPv6' : '开启 IPv6')
					])
				])
			]),

			/* 3. 拨号状态与连接信息卡片 */
			E('div', { 'class': 'mp-card' }, [
				E('div', { 'class': 'mp-card-hd' }, [
					E('h3', { 'class': 'mp-card-title' }, [
						E('span', { 'innerHTML': icons.network(st.state, 17) }),
						'数据链路连接状态'
					]),
					E('span', { 'class': 'mp-badge ' + statusBadgeCls }, statusText)
				]),
				E('div', { 'class': 'mp-kv-grid' }, [
					rowKv('连接状态', statusText),
					rowKv('驱动/通信类型', st.type_name || st.kind || '-'),
					rowKv('主机侧接口', st.host_iface || '-'),
					rowKv('生效 APN', st.apn || '（沿用模组配置）'),
					rowKv('IPv4 本机地址', st.ipv4 || '未分配'),
					rowKv('IPv6 本机地址', st.ipv6 || '未分配'),
					rowKv('已分配 CID', st.cid != null ? String(st.cid) : '-'),
					rowKv('在线持续时长', st.elapsed_sec ? (Math.floor(st.elapsed_sec / 3600) + '小时 ' + Math.floor((st.elapsed_sec % 3600) / 60) + '分') : '-')
				]),
				st.steps && st.steps.length ? E('div', { 'style': 'margin-top:14px;padding-top:10px;border-top:1px solid var(--mp-border-glass-subtle)' }, [
					E('div', { 'style': 'font-size:12.5px;font-weight:600;color:var(--mp-neutral-700);margin-bottom:6px' }, '拨号流程步骤记录:'),
					E('div', { 'style': 'display:flex;flex-direction:column;gap:4px' },
						st.steps.map(function (sp) {
							return E('div', { 'style': 'display:flex;align-items:center;gap:8px;font-size:12px' }, [
								E('span', { 'class': 'mp-badge ' + (sp.ok ? 'mp-badge-success' : 'mp-badge-danger') }, sp.ok ? '成功' : '失败'),
								E('span', { 'style': 'font-weight:500' }, sp.name),
								sp.info ? E('span', { 'style': 'color:var(--mp-neutral-500)' }, sp.info) : ''
							]);
						})
					)
				]) : ''
			]),

			/* 4. 双列布局：模组 USB 总线判定 + 内置拨号器 */
			E('div', { 'style': 'display:grid;grid-template-columns:repeat(auto-fit, minmax(320px, 1fr));gap:16px;margin-bottom:18px' }, [
				/* USB 识别 */
				E('div', { 'class': 'mp-card', 'style': 'margin-bottom:0' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [
							E('span', { 'innerHTML': icons.usb(17) }),
							'总线与驱动类型识别'
						]),
						E('span', { 'class': 'mp-badge mp-badge-primary' }, (usb.name || usb.kind || '未知').toUpperCase())
					]),
					E('div', { 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr' }, [
						rowKv('识别结论', usb.name || usb.kind || '-'),
						rowKv('识别判定来源', usb.source === 'at' ? '固件 AT 指令回读' : (usb.source === 'descriptor' ? 'USB 描述符' : '主机驱动判定')),
						rowKv('判定置信度', usb.confident ? '高（确信）' : '估算（需核验）'),
						rowKv('主机驱动提示', (usb.host_iface_hint && usb.host_iface_hint.driver) ? (usb.host_iface_hint.iface + ' (' + usb.host_iface_hint.driver + ')') : '-'),
						rowKv('出厂支持形态', 'USB: ' + ((usb.catalog_usb || []).join(' / ') || '-') + ' · PCIe: ' + ((usb.catalog_pcie || []).join(' / ') || '-'))
					]),
					usb.note ? E('p', { 'style': 'font-size:12px;color:var(--mp-warning-text);margin:10px 0 0' }, usb.note) : ''
				]),

				/* 内置拨号器 (Host Dialer) */
				E('div', { 'class': 'mp-card', 'style': 'margin-bottom:0' }, [
					E('div', { 'class': 'mp-card-hd' }, [
						E('h3', { 'class': 'mp-card-title' }, [
							E('span', { 'innerHTML': icons.chip('running', 17) }),
							'内置原生拨号后端'
						]),
						E('span', { 'class': 'mp-badge ' + (hostDialer ? 'mp-badge-success' : 'mp-badge-muted') }, hostDialer ? hostDialer.backend : '未运行')
					]),
					hostDialer ? E('div', { 'class': 'mp-kv-grid', 'style': 'grid-template-columns:1fr' }, [
						rowKv('拨号架构', hostDialer.plan || hostDialer.backend),
						rowKv('控制端口', hostDialer.control || '-'),
						rowKv('网关与 MTU', (hostDialer.gateway || '-') + ' · MTU ' + (hostDialer.mtu || 1500)),
						rowKv('模组分配 IPv4', hostDialer.modem_ipv4 || '-'),
						rowKv('DNS 服务器', (hostDialer.dns || []).join(', ') || '-'),
						rowKv('QMAP 绑定', hostDialer.qmap ? ('mux_id=' + hostDialer.qmap.mux_id + ' · v' + hostDialer.qmap.qmap_version) : '未启用')
					]) : E('div', { 'style': 'padding:20px 0;text-align:center;color:var(--mp-neutral-500);font-size:13px' },
						'当前采用 AT/ECM/RNDIS 模式，或 QMI/MBIM 内置拨号器尚未建立会话。')
				])
			]),

			/* 5. 自动组网诊断报告 */
			E('div', { 'class': 'mp-card' }, [
				E('div', { 'class': 'mp-card-hd' }, [
					E('h3', { 'class': 'mp-card-title' }, [
						E('span', { 'innerHTML': icons.pulse(17) }),
						'自动组网报告 (Netprov)'
					]),
					E('span', { 'class': 'mp-badge ' + ((autoWan && autoWan.ok) ? 'mp-badge-success' : 'mp-badge-muted') },
						autoWan ? ('第 ' + (autoWan.cycle || 1) + ' 轮 · ' + (autoWan.ok ? '正常' : '异常')) : '未运行')
				]),
				E('div', { 'class': 'mp-kv-grid' }, [
					rowKv('执行时间', autoWan ? autoWan.at : '-'),
					rowKv('生效接口', autoWan && autoWan.ifaces ? autoWan.ifaces.join(', ') : 'modem0'),
					rowKv('组网动作', autoWan && autoWan.applied && autoWan.applied.length ? autoWan.applied.join('； ') : '无变更'),
					rowKv('跳过项', autoWan && autoWan.skipped && autoWan.skipped.length ? autoWan.skipped.join('； ') : '无')
				])
			]),

			/* 6. APN 上下文与 PDP 管理卡片 */
			E('div', { 'class': 'mp-card' }, [
				E('div', { 'class': 'mp-card-hd' }, [
					E('h3', { 'class': 'mp-card-title' }, [
						E('span', { 'innerHTML': icons.settings(17) }),
						'APN 上下文与 PDP 链路管理'
					]),
					E('div', { 'class': 'mp-card-actions' }, [
						E('button', {
							'class': 'mp-btn mp-btn-primary mp-btn-sm',
							'click': function () { openApnModal(null); }
						}, [ E('span', { 'innerHTML': icons.refresh(12) }), ' 新增 APN' ])
					])
				]),
				E('table', { 'class': 'mp-table' }, [
					E('thead', {}, [
						E('tr', {}, [
							E('th', { 'style': 'width:60px' }, 'CID'),
							E('th', {}, 'APN 接入点'),
							E('th', {}, 'PDP 协议'),
							E('th', {}, '链路状态'),
							E('th', {}, '默认'),
							E('th', { 'style': 'text-align:right' }, '操作')
						])
					]),
					E('tbody', {}, (params.apns && params.apns.length) ? params.apns.map(function (a) {
						var isActive = !!a.active;
						return E('tr', {}, [
							E('td', { 'style': 'font-weight:600' }, String(a.cid)),
							E('td', { 'style': 'font-weight:500' }, a.apn || '（未设置）'),
							E('td', {}, a.pdp_type || 'IPV4V6'),
							E('td', {}, [
								E('span', { 'class': 'mp-badge ' + (isActive ? 'mp-badge-success' : 'mp-badge-muted') }, isActive ? '已激活' : '已停用')
							]),
							E('td', {}, a.default_cid ? E('span', { 'class': 'mp-badge mp-badge-primary' }, '默认') : '-'),
							E('td', { 'style': 'text-align:right' }, [
								E('div', { 'style': 'display:inline-flex;gap:6px' }, [
									E('button', {
										'class': 'mp-btn mp-btn-ghost mp-btn-sm',
										'title': isActive ? '停用此 PDP 链路' : '激活此 PDP 链路',
										'click': ui.createHandlerFn(self, function (b) {
											b.disabled = true;
											api.post('/api/params/pdp', {
												modem_id: id,
												cid: a.cid,
												active: !isActive
											}).then(function () {
												ui.addNotification(null, E('p', {}, isActive ? '已停用' : '已激活'), 'info');
												window.location.reload();
											}).catch(function (e) {
												ui.addNotification(null, E('p', {}, '操作失败: ' + (e.message || e)), 'error');
												b.disabled = false;
											});
										})
									}, isActive ? '停用' : '激活'),
									E('button', {
										'class': 'mp-btn mp-btn-ghost mp-btn-sm',
										'title': '先断开再重新激活',
										'click': ui.createHandlerFn(self, function (b) {
											b.disabled = true;
											api.post('/api/params/pdp', {
												modem_id: id,
												cid: a.cid,
												active: true,
												reconnect: true
											}).then(function () {
												ui.addNotification(null, E('p', {}, '已重新激活'), 'info');
												window.location.reload();
											}).catch(function (e) {
												ui.addNotification(null, E('p', {}, '重连失败: ' + (e.message || e)), 'error');
												b.disabled = false;
											});
										})
									}, '重连'),
									E('button', {
										'class': 'mp-btn mp-btn-ghost mp-btn-sm',
										'click': function () { openApnModal(a); }
									}, '编辑'),
									E('button', {
										'class': 'mp-btn mp-btn-danger mp-btn-sm',
										'click': ui.createHandlerFn(self, function (b) {
											if (!confirm('确定删除 CID ' + a.cid + ' 的 APN 配置吗？')) return;
											b.disabled = true;
											api.post('/api/params/apn/delete', {
												modem_id: id,
												cid: a.cid
											}).then(function () {
												ui.addNotification(null, E('p', {}, '已删除'), 'info');
												window.location.reload();
											}).catch(function (e) {
												ui.addNotification(null, E('p', {}, '删除失败: ' + (e.message || e)), 'error');
												b.disabled = false;
											});
										})
									}, '删除')
								])
							])
						]);
					}) : [
						E('tr', {}, [
							E('td', { 'colspan': 6, 'style': 'text-align:center;color:var(--mp-neutral-500);padding:18px' }, '模组内没有已保存的 APN 上下文')
						])
					])
				])
			])
		]);

		return root;
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
