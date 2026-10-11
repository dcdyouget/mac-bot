// Message block library — the single renderer for every message block on every platform.
// Spec: ../06-message-blocks.md. Platform scale comes only from CSS vars (--mb-*), set by .ctx-m / .phone.
(function () {
  const BOT = { Z: ['总管', 0, '主 Bot'], P: ['产品', 1, 'PRD、原型'], C: ['编码', 2, '写代码、部署'], T: ['测试', 3, '测试、回归'], R: ['调研', 4, '资料调研'], X: ['X 侦察', 5, '信息侦察'], B: ['比价', 6, '比价下单'] };
  const I = (n, c = '') => `<svg class="ic ${c}"><use href="#i-${n}"/></svg>`;
  const av = (k, s = 32, o = {}) => `<div class="av s${s}" style="--c:var(--avatar-${BOT[k][1]})">${k === 'Z' ? window.GLASSES : ''}${o.ring ? '<span class="ring-working"></span>' : ''}</div>`;
  const gav = (ks, s = 40) => `<div class="gav s${s} n${Math.min(ks.length, 4)}">${ks.slice(0, 4).map((k) => `<div class="av" style="--c:var(--avatar-${BOT[k][1]})"></div>`).join('')}</div>`;
  const name = (k) => k === 'me' ? '你' : BOT[k][0];
  const btn = (label, kind = 'secondary', icon) => `<button class="mb-btn ${kind}">${icon ? I(icon, 's14') : ''}${label}</button>`;

  const MB = { BOT, I, av, gav, name };

  /* ---------- inline ---------- */
  // @mention: who = bot key | 'me' | 'everyone'
  MB.at = (who) => who === 'me' ? '<span class="mb-at me">@你</span>' : who === 'everyone' ? '<span class="mb-at">@everyone</span>' : `<span class="mb-at">@${name(who)}</span>`;
  MB.code = (t) => `<code>${t}</code>`;

  /* ---------- in-bubble blocks ---------- */
  MB.quote = (k, text) => `<div class="mb-quote"><b>${name(k)}</b><span>${text}</span></div>`;

  MB.group = ({ members, title, status = 'working', note = '', created = false }) => {
    const chip = { new: '<span class="chip working">新建</span>', working: '<span class="chip working"><span class="spinner"></span>进行中</span>', review: '<span class="chip review">◎ 待验收</span>', done: '<span class="chip ok">✓ 已完成</span>' }[created ? 'new' : status];
    return `${created ? `<div class="mb-pre">${I('users', 's14')}新建了群聊</div>` : ''}<button class="mb-card mb-group">
      <div class="mb-gh">${gav(members, 40)}<div class="grow" style="min-width:0"><div class="row gap6"><b class="ellipsis">${title}</b>${chip}</div><div class="mb-sub ellipsis">群聊 · ${members.map(name).join('、')}</div></div>${I('chevR', 's14 t3')}</div>
      ${note ? `<div class="mb-gn">${note}</div>` : ''}
      <div class="mb-foot">${I('message', 's14')}进入群聊</div></button>`;
  };

  // state: open | answered | expired ; multi: checkboxes + submit
  MB.choice = ({ question, options, state = 'open', picked = [], multi = false, from }) => {
    if (state === 'answered') {
      const o = options.filter((_, i) => picked.includes(i)).map((t) => t).join('、');
      return `<div class="mb-card mb-answered">${I('check', 's14')}<span class="grow">${question}<br><b>你选择了 ${picked.map((i) => String.fromCharCode(65 + i)).join('、')} · ${o}</b></span></div>`;
    }
    return `<div class="mb-card mb-choice ${state}">
      <div class="mb-h">${I('message', 's14')}${multi ? '多选' : '单选'}${from ? ` · 来自 ${from}` : ''}${state === 'expired' ? '<span class="mb-tag">已过期</span>' : ''}</div>
      <div class="mb-q">${question}</div>
      <div class="mb-opts">${options.map((t, i) => `<button class="mb-opt ${picked.includes(i) ? 'on' : ''}"><span class="${multi ? 'mb-box' : 'mb-letter'}">${multi ? (picked.includes(i) ? I('check', 's14') : '') : String.fromCharCode(65 + i)}</span><span class="grow">${t}</span>${!multi && picked.includes(i) ? I('check', 's14') : ''}</button>`).join('')}</div>
      <div class="mb-note"><span class="grow">${multi ? `已选 ${picked.length} 项` : '也可以直接回复文字'}</span>${multi ? btn('提交', 'primary') : ''}</div></div>`;
  };

  // state: pending | allowed | always | denied
  MB.approval = ({ tool, command, from, rule, state = 'pending' }) => {
    const done = { allowed: ['success', 'check', '已允许一次'], allowed_once: ['success', 'check', '已允许一次'], always: ['success', 'check', '已设为总是允许'], always_allowed: ['success', 'check', '已设为总是允许'], denied: ['danger', 'x', '已拒绝'], expired: ['text-tertiary', 'clock', '已过期（任务已结束）'] }[state];
    return `<div class="mb-card mb-approval ${state}">
      <div class="mb-h" style="color:var(--warning)">${I('shield', 's14')}需要审批 · ${tool}</div>
      <div class="mb-b"><div class="mb-term">${command}</div><div class="mb-meta">${from} · 命中规则「${rule}」</div></div>
      ${done ? `<div class="mb-result" style="color:var(--${done[0]})">${I(done[1], 's14')}${done[2]} · 你 11:44</div>`
        : `<div class="mb-acts">${btn('允许一次', 'primary')}${btn('总是允许')}${btn('拒绝', 'danger')}</div>`}</div>`;
  };

  MB.artifacts = (list, { compact = false } = {}) => `<div class="mb-card mb-arts">${list.map(([n, p, k, ic = 'file']) =>
    `<div class="mb-art"><span class="mb-fi">${I(ic, 's16')}</span><div class="grow" style="min-width:0"><div class="row gap6"><span class="ellipsis">${n}</span><span class="by">${av(k, 16)}${name(k)}</span></div><div class="mb-path ellipsis">${p}</div></div>${compact ? '' : btn('预览', 'ghost')}</div>`).join('')}</div>`;

  // state: pending | accepted | changes
  MB.review = ({ title, list, state = 'pending' }) => `<div class="mb-card mb-review">
    <div class="mb-h" style="color:var(--review)">${I('target', 's14')}待验收 · ${title}</div>
    <div class="mb-b" style="padding-top:0">${list.map(([n, p, k, ic = 'file']) => `<div class="mb-art"><span class="mb-fi">${I(ic, 's16')}</span><div class="grow" style="min-width:0"><div class="row gap6"><span class="ellipsis">${n}</span><span class="by">${av(k, 16)}${name(k)}</span></div><div class="mb-path ellipsis">${p}</div></div></div>`).join('')}</div>
    ${state === 'accepted' || state === 'confirmed' ? `<div class="mb-result" style="color:var(--success)">${I('check', 's14')}你已确认完成 · 11:50</div>`
      : state === 'changes_requested' ? `<div class="mb-result" style="color:var(--review)">${I('edit', 's14')}你提了修改意见 · 已派给对应的 Bot</div>`
      : `<div class="mb-acts">${btn('确认完成', 'primary')}${btn('提修改意见')}${btn('进入群聊', 'ghost')}</div>`}</div>`;

  MB.handoff = (k, note = '处理（不建群）') => `<div class="mb-handoff">${I('reply', 's14')}交给 ${av(k, 16)}<b>${name(k)}</b> ${note}</div>`;

  MB.blocked = ({ reason, need }) => `<div class="mb-card">
    <div class="mb-h" style="color:var(--danger)">${I('alert', 's14')}卡住了</div>
    <div class="mb-b">${reason}<div class="mb-meta">需要：${need}</div></div>
    <div class="mb-acts">${btn('重试', 'secondary', 'refresh')}${btn('换模型', 'ghost')}</div></div>`;

  MB.takeover = ({ reason, site }) => `<div class="mb-card">
    <div class="mb-h" style="color:var(--warning)">${I('hand', 's14')}需要你接管</div>
    <div class="mb-b">${reason}<div class="mb-shot"><span class="mono">${site}</span></div></div>
    <div class="mb-acts">${btn('接管并处理', 'warn', 'hand')}</div></div>`;

  MB.dm = (a, b, n) => `<button class="mb-card mb-dm">${I('message', 's14')}<span class="grow">${av(a, 16)}<b>${name(a)}</b> 私信了 ${av(b, 16)}<b>${name(b)}</b>（${n} 条）</span><span class="mb-link">展开 ${I('chevD', 's14')}</span></button>`;

  MB.file = (n, size) => `<div class="mb-file"><span class="mb-fi lg">${I('file', 's20')}</span><div class="grow" style="min-width:0"><div class="ellipsis">${n}</div><div class="mb-meta" style="margin:0">${size}</div></div>${I('chevD', 's16 t3')}</div>`;
  MB.image = (cap) => `<div class="mb-img"><div class="ph">${I('monitor', 's24')}</div>${cap ? `<div class="mb-cap">${cap}</div>` : ''}</div>`;
  MB.link = (site, title, desc) => `<div class="mb-linkp"><div class="grow" style="min-width:0"><div class="s">${site}</div><b>${title}</b><div class="d">${desc}</div></div><div class="th"></div></div>`;
  MB.fallback = (text) => `<div class="mb-card mb-fallback">${I('alert', 's14')}<span class="grow">${text}<div class="mb-meta">这类消息需要更新客户端才能完整显示</div></span></div>`;

  /* ---------- standalone (not in a bubble) ---------- */
  // state: queued | working | waiting | done | failed | recovered
  MB.task = ({ title, state = 'working', time = '', reason = '', group, bot }) => {
    if (state === 'done') return `<div class="mb-task-line">${I('check', 's14')}<span>${bot ? name(bot) + ' · ' : ''}${title}${time ? ' · ' + time : ''}</span><button class="mb-link">详情</button></div>`;
    const ico = { queued: '<span class="t3">…</span>', working: '<span class="spinner"></span>', waiting: I('alert', 's14'), done: I('check', 's14'), failed: I('x', 's14'), recovered: I('refresh', 's14') }[state];
    const word = { queued: '排队', working: '', waiting: '等你', done: '', failed: '失败', recovered: '已恢复' }[state];
    return `<div class="mb-task ${state}"><span class="ti">${ico}</span><span class="grow ellipsis"><b>任务</b> · ${title}${group ? ` <span class="t3">· ${group}</span>` : ''} <span class="t3">· ${word ? word + ' · ' : ''}${time}${reason ? ' · ' + reason : ''}</span></span><button class="mb-btn ghost">详情</button>${state === 'working' ? '<i class="bar"></i>' : ''}</div>`;
  };
  MB.progress = (k, t) => `<div class="mb-progress"><span class="sdot"></span>${name(k)}：${t}</div>`;
  MB.system = (t) => `<div class="sysmsg">${t}</div>`;
  MB.typing = (k) => `<div class="grp"><div class="avatar-slot">${av(k)}</div><div class="stack"><div class="bubble mb-typing"><i></i><i></i><i></i></div></div></div>`;

  /* ---------- message wrapper ---------- */
  // MB.msg('C', ['text', MB.group(...)], { time, role: true, wide: [false, true] })
  // Time rules (06 §2.1): no time inside bubbles. A centered time separator appears when a message
  // is ≥ 5 min after the previous one (reset by MB.day); sender name + time sit above the group;
  // exact time shows beside a bubble on hover (desktop) / swipe-left (mobile).
  let lastMin = null;
  const toMin = (t) => { const m = /^(\d{1,2}):(\d{2})$/.exec(t || ''); return m ? +m[1] * 60 + +m[2] : null; };
  MB.resetTime = () => { lastMin = null; };
  MB.timesep = (t) => `<div class="timesep">${t}</div>`;
  MB.day = (t) => { lastMin = null; return `<div class="daysep">${t}</div>`; };
  MB.msg = (k, parts, o = {}) => {
    const out = k === 'me';
    const m = toMin(o.time);
    const sep = m !== null && (lastMin === null || m - lastMin >= 5) && o.sep !== false ? MB.timesep(o.time) : '';
    if (m !== null) lastMin = m;
    const head = !out && (o.name !== false || sep) ? `<div class="grp-head"><b>${name(k)}</b>${o.role ? `<span class="role">${BOT[k][2]}</span>` : ''}${o.time && !sep ? `<span class="t">${o.time}</span>` : ''}</div>` : '';
    // tapback reactions (02 §5.8): pinned to the last bubble; o.tapback = [[emoji, count, mine?], ...]
    const tb = o.tapback ? `<span class="tapback ${o.tapback.some((r) => r[2]) ? 'mine' : ''}">${o.tapback.map(([e, n]) => `${e}${n > 1 ? `<b>${n}</b>` : ''}`).join('')}</span>` : '';
    const bubbles = parts.map((p, i) => {
      const wide = /mb-card|mb-img|mb-file|mb-linkp/.test(p);
      return `<div class="bubble ${wide ? 'wide' : ''}">${p}${o.time ? `<span class="htime">${o.time}</span>` : ''}${i === parts.length - 1 ? tb : ''}</div>`;
    }).join('');
    const status = o.delivery ? `<div class="delivery">${o.delivery}</div>` : out && o.status ? `<div class="delivery">${o.status}</div>` : '';
    return `${sep}<div class="grp ${out ? 'out' : ''}" data-sender="${k}">${out ? '' : `<div class="avatar-slot">${av(k)}</div>`}<div class="stack">${head}${bubbles}${status}</div></div>`;
  };

  /* ---------- additional protocol blocks ---------- */
  // completion: send_msg(done) — summary + artifacts + next mentions
  MB.completion = ({ summary, artifacts = [], next = [], notifyMain = false }) =>
    `<div class="mb-done">${I('check', 's14')}<b>已完成</b> ${summary}</div>` + (artifacts.length ? MB.artifacts(artifacts) : '') +
    (next.length ? `<div class="mb-next">${next.map(([k, ins]) => `<div>${I('chevR', 's14')}${MB.at(k)} ${ins}</div>`).join('')}</div>` : '') +
    (notifyMain ? `<div class="mb-next"><div>${I('chevR', 's14')}${MB.at('Z')} 全部完成，请汇总验收</div></div>` : '');
  // approval_ref: what the group sees instead of the approval card
  MB.approval_ref = (k) => `<div class="mb-ref">${I('shield', 's14')}<span class="grow">${av(k, 16)}<b>${name(k)}</b> 在私聊里等你审批</span><button class="mb-btn secondary">前往</button></div>`;
  // loop_paused: anti-loop banner
  MB.loop_paused = ({ hops = 8, state = 'paused' }) => state === 'paused'
    ? `<div class="mb-loop">${I('stop', 's14')}<span class="grow"><b>已暂停</b> · Bot 之间连续往返 ${hops} 次</span><button class="mb-btn secondary">再允许 8 次</button><button class="mb-btn ghost">结束这一轮</button></div>`
    : `<div class="mb-loop done">${I('check', 's14')}<span class="grow">${state === 'continued' ? '你允许继续 8 次' : '你结束了这一轮'}</span></div>`;
  // takeover_request with states pending | active | done
  const takeoverBase = MB.takeover;
  MB.takeover_request = ({ reason, site, state = 'pending' }) => state === 'pending' ? takeoverBase({ reason, site })
    : `<div class="mb-card"><div class="mb-h" style="color:var(--${state === 'active' ? 'warning' : 'success'})">${I(state === 'active' ? 'hand' : 'check', 's14')}${state === 'active' ? '你正在操作 · Bot 已暂停' : '已交还给 Bot'}</div>
       <div class="mb-b">${reason}</div>${state === 'active' ? `<div class="mb-acts">${btn('回到画面', 'secondary', 'monitor')}${btn('交还给 Bot', 'primary')}</div>` : ''}</div>`;
  MB.system = (t, code) => `<div class="sysmsg"${code ? ` data-code="${code}"` : ''}>${t}</div>`;

  // protocol names (PROTOCOL 4.6) → renderers. Prototypes may use either name.
  Object.assign(MB, {
    task_card: MB.task, question: MB.choice, project_card: (o) => MB.group({ ...o, created: o.context === 'created' }),
    review_card: MB.review, delegation: MB.handoff, bot_dm_ref: MB.dm, link_preview: MB.link,
  });

  /* ---------- "/" skill picker (shared by desktop and mobile) ---------- */
  const SKILLS = [
    ['web-auth-patterns', '实现登录、注册、找回密码时使用', '我的', '14 次'],
    ['weekly-report', '周报格式与数据来源', '我的', '3 次'],
    ['agent-browser', '浏览器操作的完整说明', '内置', '9 次'],
    ['prd-template', 'PRD 模板与写作规范', '我的', ''],
    ['deploy-checklist', '部署到生产前的检查清单', '导入', ''],
  ];
  const hl = (n, q) => q ? n.replace(q, `<mark>${q}</mark>`) : n;
  const skItem = ([n, d, src, used], q, on) => `<div class="sk-item ${on ? 'on' : ''}"><span class="sk-ic">${I('zap', 's14')}</span><div class="grow" style="min-width:0"><div class="n">/${hl(n, q)}</div><div class="d ellipsis">${d}</div></div><span class="src">${src}${used ? ' · ' + used : ''}</span></div>`;
  // query '' = just typed "/" → recent + all ; otherwise filtered by substring
  MB.skillPicker = ({ query = '', sel = 0, mobile = false } = {}) => {
    const keys = mobile ? '' : `<div class="sk-foot"><span>↑↓ 选择</span><span>↵ / Tab 插入</span><span>Esc 关闭</span><span style="margin-left:auto">ϟ 技能 · 所有 Bot 共用</span></div>`;
    if (!query) {
      return `<div class="sk-h">最近使用</div>${SKILLS.slice(0, 2).map((k, i) => skItem(k, '', i === sel)).join('')}
        <div class="sk-h">全部技能 · 12</div>${SKILLS.slice(2).map((k, i) => skItem(k, '', i + 2 === sel)).join('')}${keys}`;
    }
    const hits = SKILLS.filter(([n]) => n.includes(query));
    if (!hits.length) return `<div class="col" style="align-items:center;gap:6px;padding:20px 12px;text-align:center"><span style="color:var(--text-tertiary)">${I('zap', 's20')}</span><div style="font-weight:500">没有叫「${query}」的技能</div><div class="t-foot">继续输入会作为普通文字发送</div><button class="mb-btn ghost">${I('plus', 's14')}新建技能「${query}」</button></div>${keys}`;
    return `<div class="sk-h">匹配「${query}」· ${hits.length}</div>${hits.map((k, i) => skItem(k, query, i === sel)).join('')}${keys}`;
  };
  MB.skillChip = (n, removable) => `<span class="skill-chip">${I('zap', 's14')}${n}${removable ? `<span class="x">${I('x', 's14')}</span>` : ''}</span>`;

  /* ---------- quote reply: composer bar ---------- */
  MB.replyBar = (k, excerpt, mode = 'reply') => `<div class="mb-replybar">${I(mode === 'reply' ? 'reply' : 'copy', 's18')}<div class="grow" style="min-width:0"><div class="t">${mode === 'reply' ? '回复' : '引用'} ${name(k)}${mode === 'reply' ? '' : '<span style="font-weight:400;color:var(--text-tertiary)"> · 不指定谁回应</span>'}</div><div class="e ellipsis">${excerpt}</div></div><button class="iconbtn">${I('x', 's14')}</button></div>`;

  /* ---------- question: answered states + stacked deck ---------- */
  // one question body. q = { question, options, multi, state: open|answered|expired, picked: [], text, other: bool (allow_free_text), typing }
  const letter = (i) => String.fromCharCode(65 + i);
  const qBody = (q) => {
    const { question, options, multi = false, state = 'open', picked = [], text = '', other = true, typing = '' } = q;
    if (state === 'answered') {
      const custom = !!text;
      return `<div class="mb-qa-head">${I('check', 's14')}${custom ? '你回复了' : `你选择了 ${picked.map(letter).join('、')}`}<span class="t3" style="font-weight:400;margin-left:auto">你 · 10:46</span></div>
        <div class="mb-q">${question}</div>
        <div class="mb-opts done">${options.map((t, i) => `<div class="mb-opt ${picked.includes(i) ? 'on' : 'off'}"><span class="${multi ? 'mb-box' : 'mb-letter'}">${multi ? (picked.includes(i) ? I('check', 's14') : '') : letter(i)}</span><span class="grow">${t}</span>${picked.includes(i) ? I('check', 's14') : ''}</div>`).join('')}</div>
        ${custom ? `<div class="mb-custom"><div class="l">${I('edit', 's14')}你的回复</div>${text}</div>` : ''}<div style="height:12px"></div>`;
    }
    return `<div class="mb-q" style="padding-top:10px">${question}</div>
      <div class="mb-opts">${options.map((t, i) => `<button class="mb-opt ${picked.includes(i) ? 'on' : ''}"><span class="${multi ? 'mb-box' : 'mb-letter'}">${multi ? (picked.includes(i) ? I('check', 's14') : '') : letter(i)}</span><span class="grow">${t}</span></button>`).join('')}
      ${other ? (typing ? `<div class="mb-other-input"><span class="mb-letter">${I('edit', 's14')}</span><span class="grow">${typing}<span style="border-left:1.5px solid var(--accent);margin-left:1px"></span></span><button class="mb-btn primary" style="height:26px">发送</button></div>` : `<button class="mb-opt other"><span class="mb-letter">${I('edit', 's14')}</span><span class="grow">其他，我来说…</span></button>`) : ''}</div>
      <div class="mb-note"><span class="grow">${multi ? `已选 ${picked.length} 项` : '点一项就提交'}</span>${multi ? '<button class="mb-btn primary">提交</button>' : ''}</div>`;
  };
  // deck: several question blocks in one message are stacked. cur = index shown on top.
  MB.questionDeck = ({ from, items, cur = 0 }) => {
    const done = items.filter((q) => q.state === 'answered').length, n = items.length;
    const all = done === n, layers = all ? 0 : Math.min(n - 1 - cur, 2);
    const tabs = items.map((q, i) => `<button class="mb-qtab ${i === cur ? 'on' : ''} ${q.state === 'answered' ? 'ok' : ''}">${q.state === 'answered' ? I('check', 's14') : `<b>${i + 1}</b>`}${all ? `<span>问题 ${i + 1}</span>` : ''}</button>`).join('');
    return `<div class="mb-deck l${layers}"><div class="mb-card mb-choice ${all ? 'all' : ''}">
      <div class="mb-h">${I('message', 's14')}${all ? `已回答 ${n} 个问题` : `${from} 需要你决定 · 第 ${cur + 1} / ${n} 题`}<span class="mb-tag">${all ? '' : `已答 ${done}`}</span></div>
      <div class="mb-qtabs">${!all ? `<button class="iconbtn">${I('chevL', 's14')}</button>` : ''}${tabs}${!all ? `<button class="iconbtn">${I('chevR', 's14')}</button>` : ''}</div>
      ${qBody(items[cur])}</div></div>`;
  };
  MB.questionOne = (q, from) => `<div class="mb-card mb-choice"><div class="mb-h">${I('message', 's14')}${q.state === 'answered' ? '已回答' : `${q.multi ? '多选' : '单选'} · 来自 ${from}`}</div>${qBody(q)}</div>`;

  /* ---------- waiting-message queue (not interrupting working bots) ---------- */
  // composer hint: shown while typing a message without @ in a group where bots are working
  MB.queueHint = (bots) => `<div class="mb-qhint">${I('clock', 's14')}<span class="grow">${bots.map((k) => `<b>${name(k)}</b>`).join('、')} 正在干活，这条消息不会打断${bots.length > 1 ? '它们' : '它'}，会在交付前看到</span>${bots.map((k) => `<button class="mb-btn secondary">${I('at', 's14')}${name(k)} 立即生效</button>`).join('')}</div>`;
  // queue list item (work detail panel / mobile trace page)
  MB.queueItem = (k, text, time, state = 'waiting') => `<div class="mb-qitem ${state}"><div class="row gap6">${k === 'me' ? '<b>你</b>' : `${av(k, 16)}<b>${name(k)}</b>`}<span class="t3">${time}</span><span class="grow"></span>${state === 'seen' ? `<span class="chip ok">${I('check', 's14')}已查看</span>` : `<button class="mb-btn ghost" style="height:22px;font-size:12px;padding:0 6px">改为插话</button>`}</div><div class="x">${text}</div></div>`;

  // Merge consecutive messages from the same sender into one group (02 §5.1):
  // one header on top, avatar only beside the last bubble, tail corner only on the last bubble.
  MB.mergeGroups = (root = document) => {
    root.querySelectorAll('.grp[data-sender]').forEach((g) => {
      const prev = g.previousElementSibling;
      if (!prev || !prev.matches('.grp[data-sender]') || prev.dataset.sender !== g.dataset.sender) return;
      const dst = prev.querySelector(':scope > .stack'), src = g.querySelector(':scope > .stack');
      dst.querySelectorAll(':scope > .delivery').forEach((d) => d.remove());
      src.querySelectorAll(':scope > .grp-head').forEach((h) => h.remove());
      while (src.firstChild) dst.appendChild(src.firstChild);
      g.remove();
    });
  };

  window.MB = MB;
})();
