'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.fmt as fmt';

/* 支持模块页（只读）：91 型号清单 + 逐型号能力计划。
 *
 * 全部是快接口（离线目录，不发 AT），同步拉即可。
 */

function planRows(p) {
	var rows = [];
	function r(k, v) {
		rows.push(E('tr', { 'class': 'tr' }, [
			E('td', { 'class': 'td', 'style': 'width:170px;font-weight:bold' }, k),
			E('td', { 'class': 'td', 'style': 'white-space:pre-wrap' }, v == null || v === '' ? '-' : String(v))
		]));
	}
	r('厂商', (p.vendor_name || p.vendor) + (p.in_catalog ? '' : '（不在清单里，按 ATI 识别）'));
	r('平台', (p.platform_name || p.platform || '-') +
		(p.platform_variant ? ' · ' + p.platform_variant : ''));
	if (p.platform_variant_note) r(' ', p.platform_variant_note);
	r('USB 形态', p.usb_text || (p.usb_modes || []).join(' / ') || '—');
	r('PCIe 形态', p.pcie_text || (p.pcie_modes || []).join(' / ') || '—');
	if (p.bands) {
		r('频段', 'WCDMA ' + (p.bands.wcdma || []).length +
			' · LTE ' + (p.bands.lte || []).length +
			' · NR-NSA ' + (p.bands.nr_nsa || []).length +
			' · NR-SA ' + (p.bands.nr_sa || []).length);
	}
	(p.commands || []).forEach(function (c) {
		r(c.label || c.key, (c.cmd || '') + (c.source ? '（' + c.source + '）' : '') +
			(c.on ? '  ✓' : ''));
	});
	(p.gaps || []).forEach(function (g, i) {
		r(i === 0 ? '未覆盖' : ' ', g + ((p.gap_notes || [])[i] ? '：' + p.gap_notes[i] : ''));
	});
	return rows;
}

return view.extend({
	load: function () {
		return api.get('/api/modules');
	},

	render: function (cat) {
		var self = this;
		cat = cat || {};
		var models = cat.models || [];

		var detail = E('div', { 'id': 'mods-plan', 'class': 'cbi-section' }, [
			E('p', { 'class': 'muted' }, '选择一个型号查看能力计划。')
		]);
		this._detail = detail;

		/* 按厂商分组 */
		var byVendor = {};
		models.forEach(function (m) {
			(byVendor[m.vendor_name || m.vendor] = byVendor[m.vendor_name || m.vendor] || []).push(m);
		});

		var list = E('table', { 'class': 'table' }, [
			E('tr', { 'class': 'tr table-titles' }, [
				E('th', { 'class': 'th' }, '厂商'), E('th', { 'class': 'th' }, '型号'),
				E('th', { 'class': 'th' }, '平台'), E('th', { 'class': 'th' }, 'USB'),
				E('th', { 'class': 'th' }, 'PCIe')
			])
		]);
		models.forEach(function (m) {
			list.appendChild(E('tr', { 'class': 'tr', 'style': 'cursor:pointer', 'click': ui.createHandlerFn(self, function () {
				return self.showPlan(m.model);
			}) }, [
				E('td', { 'class': 'td' }, m.vendor_name || m.vendor),
				E('td', { 'class': 'td' }, (m.model || '').toUpperCase()),
				E('td', { 'class': 'td' }, m.platform_name || m.platform || '-'),
				E('td', { 'class': 'td' }, m.usb_text || '-'),
				E('td', { 'class': 'td' }, m.pcie_text || '-')
			]));
		});

		return E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '5G 终端 · ' + '支持模块'),
			E('div', { 'class': 'cbi-map-descr' },
				'共 ' + (cat.count || models.length) + ' 个型号（' + (cat.vendor_count || '-') + ' 家厂商，' +
				(cat.pcie_count || '-') + ' 个有 PCIe 形态）。数据来自离线目录，点行查看 AT 能力计划。'),
			E('div', { 'class': 'cbi-section' }, [ list ]),
			detail
		]);
	},

	showPlan: function (model) {
		var self = this;
		return api.get('/api/modules', { model: model }).then(function (d) {
			var p = (d && d.plan) || {};
			var tbl = E('table', { 'class': 'table' }, planRows(p));
			var det = self._detail;
			det.innerHTML = '';
			det.appendChild(E('div', { 'class': 'cbi-section-title' },
				'能力计划：' + (p.model || model).toUpperCase() + (p.live ? '（在网模组）' : '')));
			det.appendChild(tbl);
			det.scrollIntoView({ behavior: 'smooth' });
		}).catch(function (e) {
			ui.addNotification(null, E('p', {}, String(e.message || e)), 'error');
		});
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
