'use strict';
'require view';
'require ui';
'require modemd.api as api';
'require modemd.store as store';
'require modemd.task as task';
'require modemd.fmt as fmt';

/* 锁网页：
 *
 *   · 状态读取（含清单频段）：异步任务 lock.status（读模组 + 生成锁频段分组）；
 *   · 锁频段 / 解锁：异步任务 lock.band（写射频寄存器，秒级，仍走任务避免占连接）。
 *
 * band_groups 由后端按「清单 ∪ 设备能力 ∪ 当前锁定值」排版（见 modem/bands.rs），
 * 前端只负责把每组渲染成勾选芯片。
 */

function groupChipset(self, g, id, current) {
	var wrap = E('div', { 'id': id, 'style': 'display:flex;flex-wrap:wrap;gap:6px' }, []);
	(g.bands || []).forEach(function (b) {
		var on = (current || []).indexOf(b) >= 0;
		var chip = E('label', {
			'style': 'border:1px solid #bbb;border-radius:12px;padding:2px 10px;cursor:pointer;user-select:none',
			'data-band': b
		}, [ String(b) ]);
		if (on) {
			chip.style.background = '#2c6e49';
			chip.style.color = '#fff';
			chip.style.borderColor = '#2c6e49';
		}
		chip.addEventListener('click', function () {
			var now = chip.style.background.indexOf('45,') >= 0 || chip.style.background === 'rgb(44, 110, 73)' || chip.dataset.on === '1';
			if (now) {
				chip.dataset.on = '';
				chip.style.background = '';
				chip.style.color = '';
				chip.style.borderColor = '';
			} else {
				chip.dataset.on = '1';
				chip.style.background = '#2c6e49';
				chip.style.color = '#fff';
				chip.style.borderColor = '#2c6e49';
			}
		});
		if (on) chip.dataset.on = '1';
		wrap.appendChild(chip);
	});
	if (!(g.bands || []).length)
		wrap.appendChild(E('span', { 'class': 'muted' }, '（无频段数据）'));
	return wrap;
}

return view.extend({
	load: function () {
		store.start(api);
		return store.fetch(api);
	},

	render: function (snap) {
		var self = this;
		var id = snap.modem_id != null ? snap.modem_id : 0;
		var cached = snap.blocks && snap.blocks.lock;

		var body = E('div', {}, []);
		var btn = E('button', {
			'class': 'btn cbi-button cbi-button-apply',
			'click': ui.createHandlerFn(self, function (b) { return self.loadLock(b, id); })
		}, '读取锁网状态');
		var applyBtn = E('button', {
			'class': 'btn cbi-button cbi-button-action',
			'click': ui.createHandlerFn(self, function () { return self.apply(id); })
		}, fmt.T('Apply'));
		var unlockBtn = E('button', {
			'class': 'btn cbi-button cbi-button-remove',
			'click': ui.createHandlerFn(self, function () { return self.unlockAll(id); })
		}, fmt.T('Unlock all'));

		var root = E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '5G 终端 · ' + fmt.T('Lock bands')),
			E('div', { 'class': 'cbi-map-descr' },
				'锁定前请确认目标频段在运营商网络内，锁错频段会导致掉网（解锁按钮可随时恢复）。'),
			E('div', { 'class': 'cbi-section-actions', 'style': 'display:flex;gap:8px' }, [ btn, applyBtn, unlockBtn ]),
			body
		]);

		this._body = body;
		this._id = id;
		if (cached) this.draw(cached);
		else this.loadLock(btn, id);
		return root;
	},

	loadLock: function (btn, id) {
		var self = this;
		btn.disabled = true;
		var orig = btn.textContent;
		btn.textContent = fmt.T('Reading');
		return task.run(api, 'lock.status', { modem_id: id, refresh: true }, { timeout: 60000 })
			.then(function (st) { self.draw(st); })
			.catch(function (e) {
				ui.addNotification(null, E('p', {}, '读取失败：' + (e.message || e)), 'error');
			})
			.finally(function () { btn.disabled = false; btn.textContent = orig; });
	},

	draw: function (st) {
		var self = this;
		var body = this._body;
		body.innerHTML = '';

		body.appendChild(E('p', { 'class': 'muted' },
			'方言：' + (st.dialect_name || st.dialect || '-') +
			(st.dialect_note ? ' — ' + st.dialect_note : '')));

		var groups = st.band_groups || [];
		if (!groups.length) {
			body.appendChild(E('p', { 'class': 'alert-message warning' },
				'本机方言不支持锁网，或还没读取到频段数据。'));
			return;
		}

		this._groups = {};
		/* 各组当前锁定值：key → band_*.bands（勾选芯片的初始状态） */
		var currents = {
			lte: (st.band_lte && st.band_lte.bands) || [],
			nr: (st.band_nr && st.band_nr.bands) || [],
			nr_nsa: (st.band_nr_nsa && st.band_nr_nsa.bands) || [],
			nr_sa: (st.band_nr && st.band_nr.bands) || []
		};
		groups.forEach(function (g) {
			var el = 'lockgrp-' + g.key;
			var sec = E('div', { 'class': 'cbi-section' }, [
				E('div', { 'class': 'cbi-section-title' }, g.title + (g.lockable ? '' : '（只读）')),
				groupChipset(self, g, el, currents[g.key] || []),
				g.note ? E('p', { 'class': 'muted', 'style': 'white-space:pre-line;margin:6px 0 0' }, g.note) : E('span', {})
			]);
			self._groups[g.key] = g;
			body.appendChild(sec);
		});

		/* 各技术域当前锁定值 / 能力（给「已锁定」说明） */
		['band_lte', 'band_nr', 'band_nr_nsa'].forEach(function (k) {
			var v = st[k];
			if (v && v.locked)
				body.appendChild(E('p', { 'class': 'muted' },
					(k === 'band_lte' ? 'LTE' : 'NR') + ' 已锁定：' + (v.bands || []).join(', ')));
		});
	},

	/* 收集勾选结果并提交（按组分开下发：每组一条任务） */
	apply: function (id) {
		var self = this;
		var jobs = [];
		var groups = this._groups || {};
		var body = this._body;
		for (var key in groups) {
			var g = groups[key];
			if (!g.lockable) continue;
			var wrap = document.getElementById('lockgrp-' + key);
			if (!wrap) continue;
			var picked = [];
			wrap.querySelectorAll('[data-band]').forEach(function (chip) {
				if (chip.dataset.on === '1') picked.push(parseInt(chip.dataset.band, 10));
			});
			/* tech 用组上的值；NSA/SA 分组时后端按组名区分 */
			jobs.push({ tech: g.tech || key, bands: picked });
		}
		if (!jobs.length)
			return Promise.reject(new Error('没有可下发的分组（本页只有只读组）'));

		return Promise.all(jobs.map(function (j) {
			return task.run(api, 'lock.band', {
				modem_id: id, tech: j.tech, bands: JSON.stringify(j.bands)
			}, { timeout: 120000 });
		})).then(function () {
			ui.addNotification(null, E('p', {}, '频段锁定已写入'), 'info');
			store.refresh();
			return self.loadLock(body.querySelector('button'), id);
		}).catch(function (e) {
			ui.addNotification(null, E('p', {}, '写入失败：' + (e.message || e)), 'error');
		});
	},

	unlockAll: function (id) {
		var self = this;
		return task.run(api, 'lock.band', { modem_id: id, unlock: true, tech: 'both' }, { timeout: 120000 })
			.then(function () {
				ui.addNotification(null, E('p', {}, fmt.T('Unlock all') + ' — ' + fmt.T('Done')), 'info');
				store.refresh();
				return self.loadLock(self._body.querySelector('button'), id);
			})
			.catch(function (e) {
				ui.addNotification(null, E('p', {}, String(e.message || e)), 'error');
			});
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
