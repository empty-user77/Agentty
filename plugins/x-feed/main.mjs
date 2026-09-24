// X Feed: collect → refine → select → convert, up to a draft that is ready to post.
//
// Agentty keeps the browser and the sign-in (cookies never reach this plugin); the plugin reads the
// profiles the user follows here, keeps each post refined with its media on disk, and turns the
// posts the user picks into drafts in the user's own styles. Posting (the last step) comes later.

import { readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { createPlugin, ui } from './agentty-plugin.mjs';
import { collect, MAX_PER_RUN } from './lib/collect.mjs';
import { checkDraft, DEFAULT_STYLES, draftMarkdown, planDrafts, rewritePrompt, sourceMarkdown } from './lib/convert.mjs';
import { createStore, DRAFT_STATUS, writeText } from './lib/store.mjs';
import { accountOf, profileUrl } from './lib/x.mjs';
import { setLanguage, t } from './lib/i18n.mjs';

const plugin = createPlugin();
const SITE = 'x';
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const state = {
  view: 'collect',
  // Collect
  newAccount: '',
  limit: 10,
  mode: 'auto',
  sites: [],
  accounts: [],
  busy: null,
  status: '',
  log: [],
  // Posts
  posts: [],
  filter: { account: 'all', day: 'all', status: 'refined', kind: 'all' },
  styleId: DEFAULT_STYLES[0].id,
  // Drafts
  drafts: [],
  draftFilter: 'open',
  openDraft: null,
  draftText: '',
  // Styles
  styles: DEFAULT_STYLES,
  editStyle: DEFAULT_STYLES[0].id,
};

let store = null;
const language = () => ({ ko: 'Korean', ja: 'Japanese', zh: 'Chinese' })[plugin.info?.language] ?? 'English';
const note = (line) => {
  state.log.unshift(`${new Date().toLocaleTimeString()} ${line}`);
  state.log.length = Math.min(state.log.length, 12);
};

// ---------------------------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------------------------

async function reload() {
  state.accounts = await store.accounts(SITE);
  state.posts = (await Promise.all(state.accounts.map((a) => store.posts(SITE, a.account)))).flat();
  state.drafts = await store.drafts();
  state.styles = await store.styles(DEFAULT_STYLES);
}

async function refreshSites() {
  try {
    state.sites = await plugin.browser.sites();
  } catch (err) {
    note(err.message);
  }
}

// ---------------------------------------------------------------------------------------------
// Collect
// ---------------------------------------------------------------------------------------------

const why = (reason) => t(`why.${reason}`);

async function addAccount(value) {
  const account = accountOf(profileUrl(value || ''));
  if (!account || !/^[A-Za-z0-9_]{1,15}$/.test(account)) {
    state.status = t('account.invalid');
    return;
  }
  const record = await store.account(SITE, account);
  await store.saveAccount(record);
  state.newAccount = '';
  state.status = t('account.added', { account });
  await reload();
}

async function collectAccount(account, { backfill = false } = {}) {
  state.busy = `@${account}`;
  state.status = t('collect.opening', { account });
  await render();
  const progress = async ({ step, run, found, index, total }) => {
    if (step === 'read') state.status = t('collect.reading', { account, found, seen: run.duplicates });
    if (step === 'save') state.status = t('collect.saving', { account, index: index + 1, total });
    await render();
  };
  const options = () => ({ limit: state.limit, mode: state.mode, backfill, progress, log: (l) => plugin.log(l) });
  let run = await collect(plugin.browser, store, account, options());
  if (run.stoppedBecause === 'signin') {
    state.status = t('sign_in.asked');
    await render();
    const answer = await plugin.browser.signIn('x.com', { message: t('sign_in.message') });
    await plugin.browser.close(run.tabId).catch(() => {});
    await refreshSites();
    if (!answer.signedIn) {
      state.status = t('sign_in.failed', { reason: answer.reason ?? '?' });
      state.busy = null;
      return;
    }
    run = await collect(plugin.browser, store, account, options());
  }
  const failed = run.failed.length ? t('collect.failed', { n: run.failed.length }) : '';
  const gap = run.gap ? t('collect.gap') : '';
  note(t('collect.log', { account, new: run.new, seen: run.duplicates, failed, why: why(run.stoppedBecause), gap }));
  state.status = run.error ? `@${account}: ${run.error}` : t('collect.result', { account, new: run.new, gap });
  state.busy = null;
  await reload();
}

async function collectAll() {
  for (const [index, record] of state.accounts.entries()) {
    await collectAccount(record.account);
    // Accounts one after another, with a person's break between them.
    if (index < state.accounts.length - 1) {
      state.status = t('collect.break');
      await render();
      await sleep(15000 + Math.random() * 20000);
    }
  }
}

// ---------------------------------------------------------------------------------------------
// Posts: refine → select
// ---------------------------------------------------------------------------------------------

function shownPosts() {
  const { account, day, status, kind } = state.filter;
  return state.posts.filter(
    (post) =>
      (account === 'all' || post.account === account) &&
      (day === 'all' || post.day === day) &&
      (status === 'all' || post.status === status) &&
      (kind === 'all' || post.kind === kind),
  );
}

async function setStatus(ids, status) {
  for (const id of ids) {
    const post = state.posts.find((p) => p.id === id);
    if (post) await store.setPostStatus(post, status);
  }
  await reload();
}

/** Picks the posts most worth converting: most liked first, originals before reposts. */
async function selectTop(count) {
  const candidates = shownPosts()
    .filter((post) => post.status === 'refined' && post.kind !== 'repost' && !post.pinned)
    .sort((a, b) => (b.metrics.likes ?? 0) - (a.metrics.likes ?? 0))
    .slice(0, count);
  await setStatus(candidates.map((p) => p.id), 'selected');
  state.status = t('select.done', { n: candidates.length });
}

// ---------------------------------------------------------------------------------------------
// Convert → drafts
// ---------------------------------------------------------------------------------------------

async function makeDrafts() {
  const style = state.styles.find((s) => s.id === state.styleId) ?? DEFAULT_STYLES[0];
  const posts = state.posts.filter((post) => post.status === 'selected');
  if (!posts.length) {
    state.status = t('select.first');
    return;
  }
  state.busy = 'drafts';
  let made = 0;
  for (const { draft, posts: group } of planDrafts(posts, style)) {
    for (const item of draft.pendingMedia) {
      const post = group.find((p) => p.id === item.postId);
      try {
        draft.media.push({ kind: item.kind, path: await store.copyToDraft(draft, post, item.path), from: post.id });
      } catch (err) {
        note(`media of ${item.postId}: ${err.message}`);
      }
    }
    delete draft.pendingMedia;
    draft.check = checkDraft(draft, style);
    await store.saveDraft(draft);
    await store.saveDraftMarkdown(draft, draftMarkdown(draft));
    for (const post of group) await store.setPostStatus(post, 'drafted', { draftId: draft.id });
    if (style.kind === 'ai') await startRewrite(draft, group, style);
    made += 1;
  }
  state.busy = null;
  state.status = t('convert.done', { n: made, style: style.name });
  await reload();
  state.view = 'drafts';
}

/** Hands a draft to an agent to write, and fills it in once the agent has written `draft.txt`. */
async function startRewrite(draft, posts, style) {
  const dir = store.draftDir(draft.id);
  await writeText(join(dir, 'source.md'), sourceMarkdown(posts));
  await rm(join(dir, 'draft.txt'), { force: true });
  try {
    // A tab of the plugin's own workspace per draft: the user sees each agent write, side by side.
    await plugin.injectPrompt({ text: rewritePrompt(style, language()), title: t('job.title', { style: style.name }), cwd: dir, target: 'own', agent: 'claude', submit: true });
    draft.ai = { state: 'writing', startedAt: new Date().toISOString() };
  } catch (err) {
    draft.ai = { state: 'failed', error: err.message };
  }
  await store.saveDraft(draft);
  if (draft.ai.state === 'writing') watchRewrite(draft.id);
}

async function watchRewrite(id) {
  const deadline = Date.now() + 15 * 60 * 1000;
  while (Date.now() < deadline) {
    await sleep(3000);
    const text = await readFile(join(store.draftDir(id), 'draft.txt'), 'utf8').catch(() => null);
    if (text === null) continue;
    // The agent may still be writing: wait until the file stops changing.
    await sleep(1500);
    const settled = await readFile(join(store.draftDir(id), 'draft.txt'), 'utf8').catch(() => text);
    const draft = await store.draft(id);
    if (!draft) return;
    draft.text = settled.trim();
    draft.ai = { ...draft.ai, state: 'done', doneAt: new Date().toISOString() };
    const style = state.styles.find((s) => s.id === draft.styleId) ?? {};
    draft.check = checkDraft(draft, style);
    await store.saveDraft(draft);
    await store.saveDraftMarkdown(draft, draftMarkdown(draft));
    note(t('convert.agent_done', { length: draft.check.length, max: draft.check.max }));
    await reload();
    return render();
  }
  const draft = await store.draft(id);
  if (draft) {
    draft.ai = { ...draft.ai, state: 'timeout' };
    await store.saveDraft(draft);
  }
}

async function updateDraft(id, change) {
  const draft = await store.draft(id);
  if (!draft) return;
  const style = state.styles.find((s) => s.id === draft.styleId) ?? {};
  const next = { ...draft, ...change };
  next.check = checkDraft(next, style);
  if (change.status && change.status !== draft.status) next.history = [...(draft.history ?? []), { at: new Date().toISOString(), status: change.status }];
  await store.saveDraft(next);
  await store.saveDraftMarkdown(next, draftMarkdown(next));
  if (change.status === 'discarded') {
    // Its posts go back to the pile they were picked from.
    for (const source of next.sources) {
      const post = await store.post(source.site, source.day, source.account, source.id);
      if (post?.draftId === id) await store.setPostStatus(post, 'selected', { draftId: null });
    }
  }
  await reload();
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

const when = (iso) => (iso ? new Date(iso).toLocaleString() : '—');
const count = (n) => (n === null || n === undefined ? '–' : n >= 10000 ? `${Math.round(n / 1000)}K` : String(n));
const STATUS_TONE = { refined: 'info', selected: 'success', skipped: 'neutral', drafted: 'warning', uploaded: 'success', draft: 'warning', ready: 'success', discarded: 'neutral' };

function renderCollect() {
  const badges = state.sites.map((site) =>
    ui.badge(
      site.signedIn
        ? site.expiresAt
          ? t('site.signed_in_until', { host: site.host, date: new Date(site.expiresAt).toLocaleDateString() })
          : t('site.signed_in', { host: site.host })
        : t('site.signed_out', { host: site.host }),
      site.signedIn ? 'success' : 'warning',
    ),
  );
  return [
    ui.row([...badges, ui.button('signin', t('sign_in'), { icon: 'key-round', variant: 'ghost' })], { gap: 'small' }),
    ui.row([ui.input('newAccount', { placeholder: t('account.placeholder'), value: state.newAccount }), ui.button('add', t('account.add'), { icon: 'plus' })], { gap: 'small' }),
    ui.row([
      ui.choice('limit', [5, 10, 20].filter((n) => n <= MAX_PER_RUN).map((n) => ({ value: String(n), label: t('per_run', { n }) })), String(state.limit)),
      ui.choice('mode', [
        { value: 'auto', label: t('mode.auto') },
        { value: 'background', label: t('mode.background') },
        { value: 'visible', label: t('mode.visible') },
      ], state.mode),
    ], { gap: 'small', wrap: true }),
    ui.button('collectAll', state.busy ? t('collect.busy', { who: state.busy }) : t('collect.all'), { icon: 'play', variant: 'primary', disabled: !!state.busy || !state.accounts.length }),
    state.busy ? ui.spinner(state.status) : ui.text(state.status || t('ready'), 'muted'),
    ui.list(
      'accounts',
      state.accounts.map((record) => {
        const last = record.lastRun;
        const total = Object.keys(record.known).length;
        return {
          id: record.account,
          title: `@${record.account}${record.gap ? t('account.gap') : ''}`,
          subtitle: last ? t('account.last', { when: when(last.at), new: last.new, seen: last.duplicates, why: why(last.stoppedBecause) }) : t('account.never'),
          detail: t('account.kept', { n: total, runs: record.runs ?? 0 }),
          icon: 'x-twitter',
          tone: record.gap ? 'warning' : last?.error ? 'error' : 'info',
          actions: [
            { id: 'collect', icon: 'play', tooltip: t('account.collect') },
            ...(record.gap ? [{ id: 'backfill', icon: 'undo-2', tooltip: t('account.backfill') }] : []),
            { id: 'folder', icon: 'folder', tooltip: t('files') },
            { id: 'remove', icon: 'trash-2', tooltip: t('account.remove') },
          ],
        };
      }),
      { empty: t('collect.none') },
    ),
    state.log.length ? ui.section(t('collect.recent'), state.log.map((line) => ui.text(line, 'small'))) : null,
  ];
}

function renderPosts() {
  const accounts = [{ value: 'all', label: t('filter.accounts') }, ...state.accounts.map((a) => ({ value: a.account, label: `@${a.account}` }))];
  const days = [...new Set(state.posts.map((p) => p.day))].sort().reverse();
  const shown = shownPosts();
  const selected = state.posts.filter((p) => p.status === 'selected').length;
  return [
    ui.row([
      ui.choice('fAccount', accounts, state.filter.account),
      ui.choice('fDay', [{ value: 'all', label: t('filter.days') }, ...days.map((d) => ({ value: d, label: d }))], state.filter.day),
    ], { gap: 'small', wrap: true }),
    ui.row([
      ui.choice('fStatus', ['all', 'refined', 'selected', 'skipped', 'drafted', 'uploaded'].map((s) => ({ value: s, label: s === 'all' ? t('filter.stage') : t(`stage.${s}`) })), state.filter.status),
      ui.choice('fKind', ['all', 'post', 'quote', 'reply', 'repost'].map((k) => ({ value: k, label: k === 'all' ? t('filter.kind') : t(`kind.${k}`) })), state.filter.kind),
    ], { gap: 'small', wrap: true }),
    ui.row([
      ui.button('selectTop', t('select.top'), { icon: 'sparkles' }),
      ui.button('selectShown', t('select.shown')),
      ui.button('clearSelection', t('select.clear'), { variant: 'ghost' }),
    ], { gap: 'small', wrap: true }),
    ui.text(t('select.count', { shown: shown.length, selected }), 'muted'),
    ui.list(
      'posts',
      shown.map((post) => {
        const images = post.media.filter((m) => m.kind === 'image' && m.path).length;
        const videos = post.media.filter((m) => m.kind !== 'image' && m.path).length;
        return {
          id: post.id,
          title: `@${post.account} · ${t(`kind.${post.kind}`)}${post.pinned ? ` · ${t('post.pinned')}` : ''} · ${post.postedAt ? new Date(post.postedAt).toLocaleDateString() : ''}`,
          subtitle: (post.text || '(no text)').replace(/\s+/g, ' ').slice(0, 160),
          detail: t('post.detail', { status: t(`stage.${post.status}`), likes: count(post.metrics.likes), reposts: count(post.metrics.reposts), images, videos }),
          icon: post.status === 'selected' ? 'circle-check' : 'file-text',
          tone: STATUS_TONE[post.status] ?? 'neutral',
          actions: [
            post.status === 'selected' ? { id: 'unselect', icon: 'circle-x', tooltip: t('post.unselect') } : { id: 'select', icon: 'circle-check', tooltip: t('post.select') },
            { id: 'skip', icon: 'x', tooltip: t('post.skip') },
            { id: 'folder', icon: 'folder', tooltip: t('files') },
          ],
        };
      }),
      { empty: t('posts.none') },
    ),
    ui.section(t('convert.title'), [
      ui.choice('style', state.styles.map((s) => ({ value: s.id, label: s.name })), state.styleId),
      ui.button('makeDrafts', t('convert.make', { n: selected }), { icon: 'wand-sparkles', variant: 'primary', disabled: !selected || !!state.busy }),
    ]),
  ];
}

function renderDrafts() {
  const open = state.drafts.find((d) => d.id === state.openDraft);
  const list = state.drafts.filter((d) => (state.draftFilter === 'open' ? d.status === 'draft' || d.status === 'ready' : d.status === state.draftFilter));
  const items = [
    ui.choice('draftFilter', [{ value: 'open', label: t('drafts.todo') }, ...DRAFT_STATUS.map((s) => ({ value: s, label: t(`stage.${s}`) }))], state.draftFilter),
    ui.list(
      'drafts',
      list.map((draft) => ({
        id: draft.id,
        title: `${draft.styleName} · ${t(`stage.${draft.status}`)}${draft.ai ? ` · ${t('drafts.ai', { state: draft.ai.state })}` : ''}`,
        subtitle: (draft.text || t('drafts.waiting')).replace(/\s+/g, ' ').slice(0, 160),
        detail: t('drafts.detail', { length: draft.check?.length ?? 0, max: draft.check?.max ?? 280, media: draft.media.length, sources: draft.sources.length }),
        icon: draft.status === 'ready' ? 'circle-check' : 'pencil',
        tone: draft.check?.problems?.length ? 'error' : STATUS_TONE[draft.status],
        actions: [{ id: 'open', icon: 'pencil', tooltip: t('draft.edit') }, { id: 'folder', icon: 'folder', tooltip: t('files') }],
      })),
      { empty: t('drafts.none') },
    ),
  ];
  if (open) {
    const check = checkDraft({ ...open, text: state.draftText });
    items.push(
      ui.section(t('draft.heading', { style: open.styleName, status: t(`stage.${open.status}`) }), [
        ui.input('draftText', { value: state.draftText, rows: 8, placeholder: t('draft.placeholder') }),
        ui.text(t('draft.length', { length: check.length, max: check.max, media: open.media.length, files: open.media.map((m) => m.path.split('/').pop()).join(', ') || t('draft.none') }), check.ok ? 'muted' : 'error'),
        check.problems.length ? ui.text(t('draft.problems', { problems: check.problems.join(', ') }), 'error') : null,
        ui.text(t('draft.from', { urls: open.sources.map((s) => s.url).join(' ') }), 'small'),
        ui.row([
          ui.button('saveDraft', t('draft.save'), { icon: 'save' }),
          open.status === 'ready'
            ? ui.button('unready', t('draft.unready'), { variant: 'secondary' })
            : ui.button('ready', t('draft.ready'), { icon: 'circle-check', variant: 'primary', disabled: !check.ok }),
          ui.button('discard', t('draft.discard'), { variant: 'danger' }),
          ui.button('closeDraft', t('draft.close'), { variant: 'ghost' }),
        ], { gap: 'small', wrap: true }),
      ]),
    );
  }
  return items;
}

function renderStyles() {
  const style = state.styles.find((s) => s.id === state.editStyle) ?? state.styles[0];
  const fields =
    style.kind === 'ai'
      ? [ui.input('sInstructions', { value: style.instructions ?? '', rows: 5, placeholder: t('style.instructions') })]
      : style.perPost
        ? [ui.input('sTemplate', { value: style.template ?? '', rows: 5, placeholder: '{text} {name} {handle} {date} {url} {hashtags} {likes}' })]
        : [
            ui.input('sHeader', { value: style.header ?? '', placeholder: t('style.header') }),
            ui.input('sItem', { value: style.item ?? '', placeholder: t('style.item') }),
            ui.input('sFooter', { value: style.footer ?? '', placeholder: t('style.footer') }),
          ];
  return [
    ui.row([ui.choice('editStyle', state.styles.map((s) => ({ value: s.id, label: s.name })), style.id), ui.button('newStyle', t('style.new'), { icon: 'plus' })], { gap: 'small', wrap: true }),
    ui.input('sName', { value: style.name, placeholder: t('style.name') }),
    ui.row([
      ui.choice('sKind', [{ value: 'template', label: t('style.template') }, { value: 'ai', label: t('style.ai') }], style.kind),
      ui.toggle('sPerPost', t('style.per_post'), !!style.perPost),
    ], { gap: 'small', wrap: true }),
    ...fields,
    ui.row([
      ui.choice('sMedia', [{ value: 'all', label: t('style.media.all') }, { value: 'first', label: t('style.media.first') }, { value: 'none', label: t('style.media.none') }], style.media ?? 'all'),
      ui.choice('sHashtags', [{ value: 'keep', label: t('style.hashtags.keep') }, { value: 'drop', label: t('style.hashtags.drop') }], style.hashtags ?? 'keep'),
      ui.choice('sMax', [280, 4000, 25000].map((n) => ({ value: String(n), label: t('style.chars', { n }) })), String(style.maxLength ?? 280)),
    ], { gap: 'small', wrap: true }),
    ui.text(t('style.fields'), 'small'),
    ui.row([ui.button('saveStyle', t('style.save'), { icon: 'save', variant: 'primary' }), ui.button('deleteStyle', t('style.delete'), { variant: 'danger' })], { gap: 'small' }),
  ];
}

function render() {
  const views = { collect: renderCollect, posts: renderPosts, drafts: renderDrafts, styles: renderStyles };
  const ready = state.drafts.filter((d) => d.status === 'ready').length;
  return plugin.setPanel(
    ui.column([
      ui.choice('view', [
        { value: 'collect', label: t('view.collect') },
        { value: 'posts', label: t('view.posts', { n: state.posts.filter((p) => p.status === 'refined' || p.status === 'selected').length }) },
        { value: 'drafts', label: t('view.drafts', { n: ready }) },
        { value: 'styles', label: t('view.styles') },
      ], state.view),
      ui.divider(),
      ...views[state.view](),
    ]),
  );
}

// ---------------------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------------------

function editStyle(change) {
  state.styles = state.styles.map((s) => (s.id === state.editStyle ? { ...s, ...change } : s));
}

async function reveal(path) {
  await plugin.revealPath(path).catch((err) => note(err.message));
}

plugin
  .onActivate(async (info) => {
    setLanguage(info.language);
    store = createStore(info.plugin.dataDir);
    await reload();
  })
  .onPanelOpen(async () => {
    await refreshSites();
    await reload();
    await render();
  })
  .onEvent('view', (e) => {
    state.view = e.value;
    return render();
  })
  .onEvent('signin', async () => {
    state.status = t('sign_in.waiting');
    await render();
    const result = await plugin.browser.signIn('x.com', { message: t('sign_in.message') });
    state.status = result.signedIn ? t('sign_in.done') : t('sign_in.failed', { reason: result.reason ?? '?' });
    await refreshSites();
    await render();
  })
  .onEvent('newAccount', async (e) => {
    state.newAccount = e.value ?? '';
    if (e.event === 'submit') {
      await addAccount(state.newAccount);
      await render();
    }
  })
  .onEvent('add', async () => {
    await addAccount(state.newAccount);
    await render();
  })
  .onEvent('limit', (e) => {
    state.limit = Math.min(Number(e.value) || 10, MAX_PER_RUN);
    return render();
  })
  .onEvent('mode', (e) => {
    state.mode = e.value;
    return render();
  })
  .onEvent('collectAll', async () => {
    if (state.busy) return;
    await collectAll();
    await render();
  })
  .onEvent('accounts', async (e) => {
    if (e.event !== 'action' || state.busy) return;
    const account = e.item;
    if (e.action === 'collect') await collectAccount(account);
    if (e.action === 'backfill') await collectAccount(account, { backfill: true });
    if (e.action === 'folder') await reveal(join(store.root, SITE));
    if (e.action === 'remove') {
      await store.removeAccount(SITE, account);
      note(t('account.removed', { account }));
      await reload();
    }
    await render();
  })
  .onEvent('fAccount', (e) => ((state.filter.account = e.value), render()))
  .onEvent('fDay', (e) => ((state.filter.day = e.value), render()))
  .onEvent('fStatus', (e) => ((state.filter.status = e.value), render()))
  .onEvent('fKind', (e) => ((state.filter.kind = e.value), render()))
  .onEvent('selectTop', async () => {
    await selectTop(3);
    await render();
  })
  .onEvent('selectShown', async () => {
    await setStatus(shownPosts().filter((p) => p.status === 'refined').map((p) => p.id), 'selected');
    await render();
  })
  .onEvent('clearSelection', async () => {
    await setStatus(state.posts.filter((p) => p.status === 'selected').map((p) => p.id), 'refined');
    await render();
  })
  .onEvent('posts', async (e) => {
    const post = state.posts.find((p) => p.id === e.item);
    if (!post) return;
    if (e.event === 'action' && e.action === 'select') await setStatus([post.id], 'selected');
    if (e.event === 'action' && e.action === 'unselect') await setStatus([post.id], 'refined');
    if (e.event === 'action' && e.action === 'skip') await setStatus([post.id], 'skipped');
    if (e.event === 'action' && e.action === 'folder') await reveal(join(store.postDir(post.site, post.day, post.account, post.id), 'post.md'));
    if (e.event === 'select') await setStatus([post.id], post.status === 'selected' ? 'refined' : 'selected');
    await render();
  })
  .onEvent('style', (e) => ((state.styleId = e.value), render()))
  .onEvent('makeDrafts', async () => {
    await makeDrafts();
    await render();
  })
  .onEvent('draftFilter', (e) => ((state.draftFilter = e.value), render()))
  .onEvent('drafts', async (e) => {
    const draft = state.drafts.find((d) => d.id === e.item);
    if (!draft) return;
    if (e.action === 'folder') await reveal(join(store.draftDir(draft.id), 'draft.md'));
    else {
      state.openDraft = draft.id;
      state.draftText = draft.text;
    }
    await render();
  })
  .onEvent('draftText', (e) => {
    state.draftText = e.value ?? '';
  })
  .onEvent('saveDraft', async () => {
    await updateDraft(state.openDraft, { text: state.draftText });
    state.status = t('draft.saved');
    await render();
  })
  .onEvent('ready', async () => {
    await updateDraft(state.openDraft, { text: state.draftText, status: 'ready' });
    await render();
  })
  .onEvent('unready', async () => {
    await updateDraft(state.openDraft, { status: 'draft' });
    await render();
  })
  .onEvent('discard', async () => {
    await updateDraft(state.openDraft, { status: 'discarded' });
    state.openDraft = null;
    await render();
  })
  .onEvent('closeDraft', () => ((state.openDraft = null), render()))
  .onEvent('editStyle', (e) => ((state.editStyle = e.value), render()))
  .onEvent('newStyle', () => {
    const id = `style-${Date.now().toString(36)}`;
    state.styles = [...state.styles, { ...DEFAULT_STYLES[0], id, name: t('style.new') }];
    state.editStyle = id;
    return render();
  })
  .onEvent('sName', (e) => editStyle({ name: e.value }))
  .onEvent('sKind', (e) => (editStyle({ kind: e.value }), render()))
  .onEvent('sPerPost', (e) => (editStyle({ perPost: !!e.value }), render()))
  .onEvent('sTemplate', (e) => editStyle({ template: e.value }))
  .onEvent('sInstructions', (e) => editStyle({ instructions: e.value }))
  .onEvent('sHeader', (e) => editStyle({ header: e.value }))
  .onEvent('sItem', (e) => editStyle({ item: e.value }))
  .onEvent('sFooter', (e) => editStyle({ footer: e.value }))
  .onEvent('sMedia', (e) => (editStyle({ media: e.value }), render()))
  .onEvent('sHashtags', (e) => (editStyle({ hashtags: e.value }), render()))
  .onEvent('sMax', (e) => (editStyle({ maxLength: Number(e.value) }), render()))
  .onEvent('saveStyle', async () => {
    await store.saveStyles(state.styles);
    state.status = t('style.saved');
    await render();
  })
  .onEvent('deleteStyle', async () => {
    if (state.styles.length <= 1) return;
    state.styles = state.styles.filter((s) => s.id !== state.editStyle);
    state.editStyle = state.styles[0].id;
    await store.saveStyles(state.styles);
    await render();
  })
  .onBrowserHidden(() => {})
  .start();
