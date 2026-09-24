// X Feed: collect → refine → select → convert, up to a draft that is ready to post; and, when
// the user turns it on, like and reply to the new posts of given accounts or keywords.
//
// Agentty keeps the browser and the sign-ins (cookies never reach this plugin). The plugin works
// in a workspace of its own where each tab is one automation: a sign-in profile (which X account),
// the accounts it collects and how often, its own browser page and its own terminals. Five tabs
// are five automations running side by side. What they collect goes to one store — per day and
// account, never twice — and is refined, picked and converted into drafts in the user's styles.

import { readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { createPlugin, ui } from './agentty-plugin.mjs';
import { collect, MAX_PER_RUN } from './lib/collect.mjs';
import { checkDraft, DEFAULT_STYLES, draftMarkdown, planDrafts, rewritePrompt, sourceMarkdown } from './lib/convert.mjs';
import {
  applyRules,
  countToday,
  fillPattern,
  likeInPage,
  pickCandidates,
  repliesPrompt,
  replyInPage,
  requiredPieces,
  sameHandle,
  sourcePath,
  throttleMs,
  toCandidate,
  waitForWindow,
  whoAmI,
} from './lib/engage.mjs';
import { createStore, day, DRAFT_STATUS, readJson, writeJson, writeText } from './lib/store.mjs';
import { accountOf, appReady, goToPath, markPinned, profileUrl, readTimeline, scrollLikeAHand } from './lib/x.mjs';
import { setLanguage, t } from './lib/i18n.mjs';

const plugin = createPlugin();
const SITE = 'x';
const HOME = 'https://x.com/home';
/** The key of the panel outside a workspace (the user put the plugin in another mode). */
const MAIN = '';
const SCHEDULES = [0, 30, 60, 180, 360, 720, 1440];
const SOURCES = ['accounts', 'top', 'latest'];
const MIN_LIKES = [0, 10, 100, 1000, 10000];
const MAX_AGES = [0, 1, 6, 24, 72];
const PER_WINDOW = [1, 2, 3, 5, 8, 12, 20];
const LIKES_A_DAY = [10, 30, 50, 100, 200];
const REPLIES_A_DAY = [5, 10, 20, 50, 100];
const PER_UNIT = [1, 3, 5, 10];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

let store = null;
/** Shared by every automation: what is on disk. */
const data = { accounts: [], posts: [], drafts: [], styles: DEFAULT_STYLES, profiles: [], profilesSupported: false };
/** Sign-in state per profile name ('default' included): [{ host, signedIn, expiresAt }]. */
const signIns = new Map();
/** Accounts some automation is reading right now: two never read the same one at once. */
const busyAccounts = new Set();
/** Each automation: its settings (on disk), its page, its run and the state of its panel. */
const autos = new Map();
/** Likes and replies per sign-in profile (one X account), shared by the automations using it. */
const engagements = new Map();
/** Posts some automation is liking or replying to right now. */
const acting = new Set();

/** An automation's settings as first made; saved ones are laid over it. */
const defaultConfig = () => ({
  title: null,
  profile: 'default',
  targets: [],
  limit: 10,
  schedule: 0,
  // Which posts: the accounts' own, or the top or latest posts found for keywords.
  source: 'accounts',
  keywords: [],
  minLikes: 0,
  maxAgeHours: 24,
  tasks: { collect: true, like: false, reply: false },
  reply: { pattern: '', ai: false, instructions: '', required: '', maxLength: 280 },
  // A random pause of minMs..maxMs between actions, at most 1..maxPer10Min actions (drawn anew)
  // every 10 minutes, and daily caps per sign-in.
  pace: { minMs: 100, maxMs: 5000, maxPer10Min: 3, likesPerDay: 50, repliesPerDay: 20, perUnit: 3 },
});

function withDefaults(saved) {
  const base = defaultConfig();
  if (!saved) return base;
  return {
    ...base,
    ...saved,
    tasks: { ...base.tasks, ...saved.tasks },
    reply: { ...base.reply, ...saved.reply },
    pace: { ...base.pace, ...saved.pace },
  };
}

const language = () => ({ ko: 'Korean', ja: 'Japanese', zh: 'Chinese' })[plugin.info?.language] ?? 'English';

function auto(instance) {
  let a = autos.get(instance);
  if (!a) {
    a = {
      instance,
      config: defaultConfig(),
      tabId: null,
      window: {},
      busy: false,
      stop: false,
      status: '',
      log: [],
      timer: null,
      nextAt: null,
      ui: {
        view: 'automation',
        newTarget: '',
        newKeyword: '',
        newProfile: '',
        filter: { account: 'all', day: 'all', status: 'refined', kind: 'all' },
        styleId: DEFAULT_STYLES[0].id,
        draftFilter: 'open',
        openDraft: null,
        draftText: '',
        editStyle: DEFAULT_STYLES[0].id,
      },
    };
    autos.set(instance, a);
  }
  return a;
}

const note = (a, line) => {
  a.log.unshift(`${new Date().toLocaleTimeString()} ${line}`);
  a.log.length = Math.min(a.log.length, 12);
};
const engineKey = (instance) => instance || 'main';

// ---------------------------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------------------------

async function reload() {
  data.accounts = await store.accounts(SITE);
  data.posts = (await Promise.all(data.accounts.map((a) => store.posts(SITE, a.account)))).flat();
  data.drafts = await store.drafts();
  data.styles = await store.styles(DEFAULT_STYLES);
}

async function refreshProfiles() {
  try {
    const { supported, profiles } = await plugin.browser.profiles();
    data.profilesSupported = supported;
    data.profiles = profiles;
  } catch {
    data.profiles = [];
  }
}

async function refreshSignIn(profile) {
  try {
    signIns.set(profile, await plugin.browser.sites({ profile }));
  } catch {
    // Not allowed the browser yet, or no window: the badge says nothing.
  }
}

const signedIn = (profile) => (signIns.get(profile) ?? []).some((site) => site.signedIn);

/**
 * The user may sign in (or out) right in an automation's page, without the sign-in button: the
 * profiles in use are looked at again — every few seconds while one is signed out, every minute
 * otherwise — and the panels follow.
 */
let watching = false;
function watchSignIns() {
  if (watching) return;
  watching = true;
  let tick = 0;
  const look = async () => {
    tick += 1;
    const profiles = new Set([...autos.values()].map((a) => a.config.profile));
    let changed = false;
    for (const profile of profiles) {
      if (signedIn(profile) && tick % 12 !== 0) continue;
      const before = JSON.stringify(signIns.get(profile) ?? null);
      await refreshSignIn(profile);
      changed ||= JSON.stringify(signIns.get(profile) ?? null) !== before;
    }
    if (changed) await renderAll();
    setTimeout(look, 5000);
  };
  setTimeout(look, 5000);
}

// ---------------------------------------------------------------------------------------------
// An automation: its page, its runs, its clock
// ---------------------------------------------------------------------------------------------

/** The automation's own page, in its profile; opened again when it went. */
async function ensurePage(a) {
  if (a.tabId !== null) {
    try {
      await plugin.browser.info(a.tabId);
      return a.tabId;
    } catch {
      a.tabId = null;
    }
  }
  const { tabId } = await plugin.browser.open(HOME, { profile: a.config.profile, instance: a.instance || undefined });
  a.tabId = tabId;
  return tabId;
}

async function openAutomation(instance, title) {
  const a = auto(instance);
  const saved = await store.engine(engineKey(instance));
  if (saved) a.config = withDefaults(saved);
  if (!a.config.title && title) a.config.title = title;
  await refreshSignIn(a.config.profile);
  await engagementOf(a.config.profile);
  try {
    await ensurePage(a);
  } catch (err) {
    note(a, err.message);
  }
  schedule(a);
  await render(instance);
}

async function closeAutomation(instance) {
  const a = autos.get(instance);
  if (!a) return;
  a.stop = true;
  clearTimeout(a.timer);
  autos.delete(instance);
  // Its settings go with its tab; what it collected stays in the shared store.
  await store.removeEngine(engineKey(instance));
}

async function saveConfig(a) {
  await store.saveEngine(engineKey(a.instance), a.config);
  if (a.instance && a.config.title) await plugin.setInstanceTitle(a.instance, a.config.title).catch(() => {});
}

/** Sets the next run from the automation's interval, never exactly the same twice. */
function schedule(a) {
  clearTimeout(a.timer);
  a.timer = null;
  a.nextAt = null;
  const minutes = a.config.schedule;
  if (!minutes) return;
  const delay = minutes * 60000 * (0.85 + Math.random() * 0.3);
  a.nextAt = Date.now() + delay;
  a.timer = setTimeout(() => runAutomation(a.instance).catch((err) => plugin.log(err.stack ?? String(err))), delay);
}

/** What a run goes through: the accounts, or the keywords. */
const unitsOf = (c) => (c.source === 'accounts' ? c.targets : c.keywords);
const engaging = (c) => c.tasks.like || c.tasks.reply;
const collecting = (c) => c.source === 'accounts' && c.tasks.collect;
const unitName = (c, unit) => (c.source === 'accounts' ? `@${unit}` : unit);

async function runAutomation(instance) {
  const a = autos.get(instance);
  if (!a || a.busy) return;
  const c = a.config;
  const units = unitsOf(c);
  if (!units.length) {
    a.status = c.source === 'accounts' ? t('auto.no_targets') : t('src.keywords.none');
    return render(instance);
  }
  if (!collecting(c) && !engaging(c)) {
    a.status = t('act.none');
    return render(instance);
  }
  a.busy = true;
  a.stop = false;
  let total = 0;
  let accounts = 0;
  const done = { like: 0, reply: 0 };
  try {
    const tabId = await ensurePage(a);
    a.self = null;
    for (const [index, unit] of units.entries()) {
      if (a.stop) break;
      if (collecting(c)) {
        if (busyAccounts.has(unit)) {
          note(a, t('auto.busy_elsewhere', { account: unit }));
        } else {
          busyAccounts.add(unit);
          try {
            const run = await collectOne(a, unit, tabId);
            total += run?.new ?? 0;
            accounts += 1;
          } finally {
            busyAccounts.delete(unit);
          }
        }
      }
      if (engaging(c) && !a.stop) {
        const result = await engageUnit(a, unit, tabId);
        done.like += result.like;
        done.reply += result.reply;
        if (result.signin || result.capped) break;
      }
      // One after another, with a person's break between them.
      if (index < units.length - 1 && !a.stop) {
        a.status = t('collect.break');
        await render(instance);
        await rest(a, 15000 + Math.random() * 20000);
      }
    }
    const parts = [];
    if (collecting(c)) parts.push(t('auto.done', { new: total, accounts }));
    if (engaging(c)) parts.push(t('eng.done', { likes: done.like, replies: done.reply }));
    if (a.status !== t('sign_in.asked')) a.status = parts.join(' ');
  } catch (err) {
    a.status = err.message;
    plugin.log(err.stack ?? String(err));
  } finally {
    a.busy = false;
    a.stop = false;
    schedule(a);
    await reload();
    await renderAll();
  }
}

/** Waits `ms`, a second at a time, and says false when the automation was stopped meanwhile. */
async function rest(a, ms) {
  const end = Date.now() + ms;
  while (!a.stop && Date.now() < end) await sleep(Math.min(1000, end - Date.now()));
  return !a.stop;
}

/** Moves the automation's page to `path` as a click would; from home when X's app is not up. */
async function goTo(tabId, path) {
  const ready = await plugin.browser.eval(tabId, appReady).catch(() => false);
  const moved = ready && (await plugin.browser.eval(tabId, goToPath, { path }).catch(() => false));
  if (!moved) {
    await plugin.browser.navigate(tabId, HOME);
    await plugin.browser.wait(tabId, { timeoutMs: 30000 });
    await sleep(1500 + Math.random() * 2000);
    await plugin.browser.eval(tabId, goToPath, { path });
  }
  await sleep(2500 + Math.random() * 2500);
}

// ---------------------------------------------------------------------------------------------
// Engaging: likes and replies, paced
// ---------------------------------------------------------------------------------------------

async function engagementOf(profile) {
  if (!engagements.has(profile)) engagements.set(profile, await store.engagement(profile));
  return engagements.get(profile);
}

/** The newest posts of one account or keyword worth acting on, a few reads deep. */
async function findPosts(a, unit, tabId, state) {
  const c = a.config;
  const want = c.pace.perUnit;
  const seen = [];
  const found = [];
  let picked = [];
  for (let reads = 0; reads < 4; reads += 1) {
    const page = await plugin.browser.eval(tabId, readTimeline, { seen }, { timeoutMs: 20000 });
    if (page.signInWall && !seen.length) return { signin: true, posts: [] };
    let reachedDone = false;
    for (const raw of reads === 0 && c.source === 'accounts' ? markPinned(page.posts) : page.posts) {
      seen.push(raw.id);
      const post = toCandidate(raw);
      if (c.source === 'accounts') {
        if (post.repost || !sameHandle(post.handle, unit)) continue;
        // An account's timeline is newest first: past a post acted on, the rest was seen before.
        if (!post.pinned && state.engaged[post.id]) {
          reachedDone = true;
          break;
        }
      }
      found.push(post);
    }
    picked = pickCandidates(found, {
      minLikes: c.minLikes,
      maxAgeHours: c.maxAgeHours,
      engaged: new Set([...Object.keys(state.engaged), ...acting]),
      self: a.self,
    });
    if (reachedDone || picked.length >= want || page.atEnd) break;
    await plugin.browser.eval(tabId, scrollLikeAHand);
    await sleep(2500 + Math.random() * 3500);
  }
  return { signin: false, posts: picked.slice(0, want) };
}

async function engageUnit(a, unit, tabId) {
  const c = a.config;
  const result = { like: 0, reply: 0, signin: false, capped: false };
  const state = await engagementOf(c.profile);
  a.status = t('eng.finding', { unit: unitName(c, unit) });
  await render(a.instance);
  await goTo(tabId, sourcePath(c.source, unit));
  if (!a.self) a.self = await plugin.browser.eval(tabId, whoAmI).catch(() => null);
  const { signin, posts } = await findPosts(a, unit, tabId, state);
  if (signin) {
    a.status = t('sign_in.asked');
    result.signin = true;
    return result;
  }
  if (!posts.length) {
    note(a, `${unitName(c, unit)}: ${t('eng.none')}`);
    return result;
  }
  for (const post of posts) acting.add(post.id);
  try {
    const replies = c.tasks.reply ? await writeReplies(a, posts) : new Map();
    for (const post of posts) {
      if (a.stop) break;
      const done = await actOn(a, tabId, post, replies.get(post.id), state);
      result.like += done.like;
      result.reply += done.reply;
      if (done.capped) {
        result.capped = true;
        break;
      }
    }
  } finally {
    for (const post of posts) acting.delete(post.id);
  }
  return result;
}

/** Each post's reply: the agent's (when on) or the pattern's, then made to follow the rules. */
async function writeReplies(a, posts) {
  const r = a.config.reply;
  const required = requiredPieces(r.required);
  const written = r.ai ? await agentReplies(a, posts, required) : new Map();
  const replies = new Map();
  for (const post of posts) {
    const checked = applyRules(written.get(post.id) || fillPattern(r.pattern, post), { required, maxLength: r.maxLength });
    replies.set(post.id, checked);
    if (!checked.ok) note(a, t('eng.no_reply', { handle: post.handle ?? post.id, problems: checked.problems.join(', ') }));
  }
  return replies;
}

/** Asks an agent, beside this automation's terminals, for one reply per post (`replies.json`). */
async function agentReplies(a, posts, required) {
  const r = a.config.reply;
  const dir = store.engageDir(`${Date.now().toString(36)}-${engineKey(a.instance)}`);
  await writeJson(join(dir, 'posts.json'), posts.map(({ id, author, handle, text, url }) => ({ id, author, handle, text, url })));
  a.status = t('eng.writing', { n: posts.length });
  await render(a.instance);
  try {
    await plugin.injectPrompt({
      text: repliesPrompt({ language: language(), instructions: r.instructions, pattern: r.pattern, required, maxLength: r.maxLength }),
      title: t('eng.writing', { n: posts.length }),
      cwd: dir,
      target: 'own',
      instance: a.instance || undefined,
      agent: 'claude',
      submit: true,
    });
  } catch (err) {
    note(a, t('eng.agent_failed', { reason: err.message }));
    return new Map();
  }
  const deadline = Date.now() + 5 * 60 * 1000;
  const ids = new Set(posts.map((p) => p.id));
  while (Date.now() < deadline && !a.stop) {
    await sleep(3000);
    const list = await readJson(join(dir, 'replies.json'), null);
    if (!Array.isArray(list)) continue;
    return new Map(list.filter((e) => e && ids.has(String(e.id)) && typeof e.text === 'string').map((e) => [String(e.id), e.text.trim()]));
  }
  note(a, t('eng.agent_failed', { reason: a.stop ? t('auto.stop') : 'timeout' }));
  return new Map();
}

/**
 * Waits for the pace: the 10-minute window's cap first, then a random pause. False when the
 * automation was stopped meanwhile.
 */
async function paceWait(a, state) {
  const p = a.config.pace;
  for (;;) {
    if (a.stop) return false;
    const wait = waitForWindow(a.window, state.log, { maxPerWindow: p.maxPer10Min });
    if (!wait) break;
    a.status = t('eng.window', { when: new Date(Date.now() + wait).toLocaleTimeString() });
    await render(a.instance);
    if (!(await rest(a, wait))) return false;
  }
  a.status = t('eng.pausing');
  await render(a.instance);
  return rest(a, throttleMs(p));
}

/** Likes and/or replies to one post on its own page, within the daily caps. */
async function actOn(a, tabId, post, reply, state) {
  const c = a.config;
  const done = { like: 0, reply: 0, capped: false };
  const who = post.handle ?? post.id;
  const record = (action, ok, extra = {}) => {
    state.log.push({ at: Date.now(), day: day(), action, ok, id: post.id, handle: post.handle, url: post.url, automation: c.title || a.instance, ...extra });
    note(a, ok ? t(action === 'like' ? 'eng.liked' : 'eng.replied', { handle: who }) : t('eng.failed', { action: t(`act.${action}`), handle: who, error: extra.error ?? '?' }));
  };
  const today = countToday(state.log);
  const likesLeft = today.like < c.pace.likesPerDay;
  const repliesLeft = today.reply < c.pace.repliesPerDay;
  const wantLike = c.tasks.like && likesLeft;
  const wantReply = c.tasks.reply && repliesLeft && reply?.ok;
  if ((!c.tasks.like || !likesLeft) && (!c.tasks.reply || !repliesLeft)) {
    note(a, t('eng.daily', { action: [c.tasks.like && t('act.like'), c.tasks.reply && t('act.reply')].filter(Boolean).join(', ') }));
    done.capped = true;
    return done;
  }
  if (!wantLike && !wantReply) return done;
  await goTo(tabId, new URL(post.url).pathname);
  const engaged = { at: Date.now() };
  if (wantLike && (await paceWait(a, state))) {
    const r = await plugin.browser.eval(tabId, likeInPage, { id: post.id }, { timeoutMs: 20000 }).catch((err) => ({ error: err.message }));
    if (r.liked) {
      done.like = 1;
      engaged.like = true;
      record('like', true);
    } else if (r.already) {
      engaged.like = true;
    } else {
      record('like', false, { error: r.error ?? (r.found ? 'no like button' : 'post not on the page') });
    }
  }
  if (wantReply && (await paceWait(a, state))) {
    const r = await plugin.browser.eval(tabId, replyInPage, { text: reply.text }, { timeoutMs: 30000 }).catch((err) => ({ error: err.message }));
    if (r.sent) {
      done.reply = 1;
      engaged.reply = reply.text;
      record('reply', true, { text: reply.text });
    } else {
      record('reply', false, { error: r.error });
    }
  }
  // Tried once (not cut short by Stop): never again, whatever happened.
  if (engaged.like || engaged.reply || !a.stop) state.engaged[post.id] = engaged;
  await store.saveEngagement(c.profile, state);
  await render(a.instance);
  return done;
}

async function collectOne(a, account, tabId) {
  const progress = async ({ step, run, found, index, total }) => {
    if (step === 'open') a.status = t('collect.opening', { account });
    if (step === 'read') a.status = t('collect.reading', { account, found, seen: run.duplicates });
    if (step === 'save') a.status = t('collect.saving', { account, index: index + 1, total });
    await render(a.instance);
  };
  const options = () => ({
    limit: a.config.limit,
    tabId,
    profile: a.config.profile,
    instance: a.instance || undefined,
    engine: a.config.title || a.instance,
    progress,
    log: (line) => plugin.log(line),
  });
  let run = await collect(plugin.browser, store, account, options());
  if (run.stoppedBecause === 'signin') {
    a.status = t('sign_in.asked');
    await render(a.instance);
    const answer = await plugin.browser.signIn('x.com', { message: t('sign_in.message'), profile: a.config.profile, instance: a.instance || undefined });
    await refreshSignIn(a.config.profile);
    if (!answer.signedIn) {
      a.status = t('sign_in.failed', { reason: answer.reason ?? '?' });
      return run;
    }
    run = await collect(plugin.browser, store, account, options());
  }
  const failed = run.failed.length ? t('collect.failed', { n: run.failed.length }) : '';
  const gap = run.gap ? t('collect.gap') : '';
  note(a, t('collect.log', { account, new: run.new, seen: run.duplicates, failed, why: t(`why.${run.stoppedBecause}`), gap }));
  return run;
}

// ---------------------------------------------------------------------------------------------
// Posts: refine → select; drafts: convert
// ---------------------------------------------------------------------------------------------

function shownPosts(a) {
  const { account, day, status, kind } = a.ui.filter;
  return data.posts.filter(
    (post) =>
      (account === 'all' || post.account === account) &&
      (day === 'all' || post.day === day) &&
      (status === 'all' || post.status === status) &&
      (kind === 'all' || post.kind === kind),
  );
}

async function setStatus(ids, status) {
  for (const id of ids) {
    const post = data.posts.find((p) => p.id === id);
    if (post) await store.setPostStatus(post, status);
  }
  await reload();
}

async function makeDrafts(a) {
  const style = data.styles.find((s) => s.id === a.ui.styleId) ?? DEFAULT_STYLES[0];
  const posts = data.posts.filter((post) => post.status === 'selected');
  if (!posts.length) {
    a.status = t('select.first');
    return;
  }
  let made = 0;
  for (const { draft, posts: group } of planDrafts(posts, style)) {
    for (const item of draft.pendingMedia) {
      const post = group.find((p) => p.id === item.postId);
      try {
        draft.media.push({ kind: item.kind, path: await store.copyToDraft(draft, post, item.path), from: post.id });
      } catch (err) {
        note(a, err.message);
      }
    }
    delete draft.pendingMedia;
    draft.check = checkDraft(draft, style);
    await store.saveDraft(draft);
    await store.saveDraftMarkdown(draft, draftMarkdown(draft));
    for (const post of group) await store.setPostStatus(post, 'drafted', { draftId: draft.id });
    if (style.kind === 'ai') await startRewrite(a, draft, group, style);
    made += 1;
  }
  a.status = t('convert.done', { n: made, style: style.name });
  await reload();
  a.ui.view = 'drafts';
}

/** Hands a draft to an agent, beside this automation's terminals; fills it in from `draft.txt`. */
async function startRewrite(a, draft, posts, style) {
  const dir = store.draftDir(draft.id);
  await writeText(join(dir, 'source.md'), sourceMarkdown(posts));
  await rm(join(dir, 'draft.txt'), { force: true });
  try {
    await plugin.injectPrompt({
      text: rewritePrompt(style, language()),
      title: t('job.title', { style: style.name }),
      cwd: dir,
      target: 'own',
      instance: a.instance || undefined,
      agent: 'claude',
      submit: true,
    });
    draft.ai = { state: 'writing', startedAt: new Date().toISOString() };
  } catch (err) {
    draft.ai = { state: 'failed', error: err.message };
  }
  await store.saveDraft(draft);
  if (draft.ai.state === 'writing') watchRewrite(a, draft.id);
}

async function watchRewrite(a, id) {
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
    draft.check = checkDraft(draft, data.styles.find((s) => s.id === draft.styleId) ?? {});
    await store.saveDraft(draft);
    await store.saveDraftMarkdown(draft, draftMarkdown(draft));
    note(a, t('convert.agent_done', { length: draft.check.length, max: draft.check.max }));
    await reload();
    return renderAll();
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
  const next = { ...draft, ...change };
  next.check = checkDraft(next, data.styles.find((s) => s.id === draft.styleId) ?? {});
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
// Rendering (one panel per automation)
// ---------------------------------------------------------------------------------------------

const when = (ms) => (ms ? new Date(ms).toLocaleString() : '—');
const count = (n) => (n === null || n === undefined ? '–' : n >= 10000 ? `${Math.round(n / 1000)}K` : String(n));
const STATUS_TONE = { refined: 'info', selected: 'success', skipped: 'neutral', drafted: 'warning', uploaded: 'success', draft: 'warning', ready: 'success', discarded: 'neutral' };
const scheduleLabel = (n) => (!n ? t('auto.schedule.manual') : n < 60 ? t('auto.schedule.minutes', { n }) : t('auto.schedule.hours', { n: n / 60 }));

function signInBadges(profile) {
  const sites = signIns.get(profile) ?? [];
  return sites.map((site) =>
    ui.badge(
      site.signedIn
        ? site.expiresAt
          ? t('site.signed_in_until', { host: site.host, date: new Date(site.expiresAt).toLocaleDateString() })
          : t('site.signed_in', { host: site.host })
        : t('site.signed_out', { host: site.host }),
      site.signedIn ? 'success' : 'warning',
    ),
  );
}

function renderTargets(a) {
  const c = a.config;
  return [
    ui.row([ui.input('newTarget', { placeholder: t('account.placeholder'), value: a.ui.newTarget }), ui.button('addTarget', t('account.add'), { icon: 'plus' })], { gap: 'small' }),
    ui.list(
      'targets',
      c.targets.map((account) => {
        const record = data.accounts.find((r) => r.account === account);
        const last = record?.lastRun;
        return {
          id: account,
          title: `@${account}${record?.gap ? t('account.gap') : ''}`,
          subtitle: last ? t('account.last', { when: new Date(last.at).toLocaleString(), new: last.new, seen: last.duplicates, why: t(`why.${last.stoppedBecause}`) }) : t('account.never'),
          detail: record ? t('account.kept', { n: Object.keys(record.known).length, runs: record.runs ?? 0 }) : undefined,
          icon: 'x-twitter',
          tone: record?.gap ? 'warning' : last?.error ? 'error' : 'info',
          actions: [
            ...(record?.gap ? [{ id: 'backfill', icon: 'undo-2', tooltip: t('account.backfill') }] : []),
            { id: 'folder', icon: 'folder', tooltip: t('files') },
            { id: 'remove', icon: 'x', tooltip: t('auto.target.remove') },
          ],
        };
      }),
      { empty: t('auto.targets.none') },
    ),
  ];
}

function renderKeywords(a) {
  return [
    ui.row([ui.input('newKeyword', { placeholder: t('src.keyword_placeholder'), value: a.ui.newKeyword }), ui.button('addKeyword', t('account.add'), { icon: 'plus' })], { gap: 'small' }),
    ui.list(
      'keywords',
      a.config.keywords.map((keyword) => ({ id: keyword, title: keyword, icon: 'search', tone: 'info', actions: [{ id: 'remove', icon: 'x', tooltip: t('auto.target.remove') }] })),
      { empty: t('src.keywords.none') },
    ),
  ];
}

function renderEngaging(a) {
  const c = a.config;
  const items = [];
  if (c.tasks.reply) {
    items.push(
      ui.section(t('reply.title'), [
        ui.text(t('reply.pattern'), 'small'),
        ui.input('rPattern', { value: c.reply.pattern, rows: 3 }),
        ui.toggle('rAi', t('reply.ai'), c.reply.ai),
        c.reply.ai ? ui.input('rInstructions', { value: c.reply.instructions, rows: 3, placeholder: t('reply.instructions') }) : null,
        ui.text(t('reply.required'), 'small'),
        ui.input('rRequired', { value: c.reply.required, rows: 3 }),
      ]),
    );
  }
  const state = engagements.get(c.profile);
  const today = state ? countToday(state.log) : { like: 0, reply: 0 };
  items.push(
    ui.section(t('safe.title'), [
      ui.text(t('safe.delay'), 'small'),
      ui.row([ui.input('paceMin', { value: String(c.pace.minMs), placeholder: t('safe.min') }), ui.input('paceMax', { value: String(c.pace.maxMs), placeholder: t('safe.max') })], { gap: 'small' }),
      ui.choice('perWindow', PER_WINDOW.map((n) => ({ value: String(n), label: t('safe.window', { n }) })), String(c.pace.maxPer10Min)),
      ui.row([
        c.tasks.like ? ui.choice('likesDay', LIKES_A_DAY.map((n) => ({ value: String(n), label: t('safe.likes_day', { n }) })), String(c.pace.likesPerDay)) : null,
        c.tasks.reply ? ui.choice('repliesDay', REPLIES_A_DAY.map((n) => ({ value: String(n), label: t('safe.replies_day', { n }) })), String(c.pace.repliesPerDay)) : null,
      ], { gap: 'small', wrap: true }),
      ui.choice('perUnit', PER_UNIT.map((n) => ({ value: String(n), label: t('safe.per_unit', { n }) })), String(c.pace.perUnit)),
      ui.text(t('eng.today', { likes: today.like, replies: today.reply }), 'muted'),
    ]),
  );
  return items;
}

function renderAutomation(a) {
  const c = a.config;
  const profiles = [{ value: 'default', label: t('auto.profile.default') }, ...data.profiles.map((p) => ({ value: p, label: p }))];
  const ready = unitsOf(c).length > 0 && (collecting(c) || engaging(c));
  return [
    ui.input('title', { placeholder: t('auto.name'), value: c.title ?? '' }),
    ui.section(t('auto.profile'), [
      ui.choice('profile', profiles, c.profile),
      data.profilesSupported
        ? ui.row([ui.input('newProfile', { placeholder: t('auto.profile.placeholder'), value: a.ui.newProfile }), ui.button('addProfile', t('auto.profile.add'), { icon: 'plus' })], { gap: 'small' })
        : ui.text(t('auto.profile.unsupported'), 'small'),
      // The button only while there is something to do: signed in, the badge says it all.
      ui.row([...signInBadges(c.profile), signedIn(c.profile) ? null : ui.button('signin', t('sign_in'), { icon: 'key-round', variant: 'ghost' })], { gap: 'small' }),
      c.profile !== 'default' ? ui.button('removeProfile', t('auto.profile.remove'), { variant: 'danger' }) : null,
    ]),
    ui.section(t('src.title'), [
      ui.choice('source', SOURCES.map((s) => ({ value: s, label: t(`src.${s}`) })), c.source),
      ...(c.source === 'accounts' ? renderTargets(a) : renderKeywords(a)),
      ui.row([
        ui.choice('minLikes', MIN_LIKES.map((n) => ({ value: String(n), label: n ? t('src.min_likes', { n }) : t('src.any_likes') })), String(c.minLikes)),
        ui.choice('maxAge', MAX_AGES.map((n) => ({ value: String(n), label: n ? t('src.max_age', { n }) : t('src.any_age') })), String(c.maxAgeHours)),
      ], { gap: 'small', wrap: true }),
    ]),
    ui.section(t('act.title'), [
      c.source === 'accounts' ? ui.toggle('tCollect', t('act.collect'), c.tasks.collect) : null,
      ui.toggle('tLike', t('act.like'), c.tasks.like),
      ui.toggle('tReply', t('act.reply'), c.tasks.reply),
      engaging(c) ? ui.text(t('act.warning'), 'error') : null,
    ]),
    ...(engaging(c) ? renderEngaging(a) : []),
    ui.section(t('auto.schedule'), [
      ui.choice('schedule', SCHEDULES.map((n) => ({ value: String(n), label: scheduleLabel(n) })), String(c.schedule)),
      collecting(c) ? ui.choice('limit', [5, 10, 20].filter((n) => n <= MAX_PER_RUN).map((n) => ({ value: String(n), label: t('per_run', { n }) })), String(c.limit)) : null,
      a.nextAt ? ui.text(t('auto.next', { when: when(a.nextAt) }), 'small') : null,
    ]),
    ui.row([
      a.busy ? ui.button('stop', a.stop ? t('auto.stopping') : t('auto.stop'), { icon: 'circle-x', variant: 'danger', disabled: a.stop }) : ui.button('run', t('auto.run'), { icon: 'play', variant: 'primary', disabled: !ready }),
    ]),
    a.busy ? ui.spinner(a.status) : ui.text(a.status || t('auto.idle'), 'muted'),
    a.log.length ? ui.section(t('collect.recent'), a.log.map((line) => ui.text(line, 'small'))) : null,
  ];
}

function renderPosts(a) {
  const accounts = [{ value: 'all', label: t('filter.accounts') }, ...data.accounts.map((r) => ({ value: r.account, label: `@${r.account}` }))];
  const days = [...new Set(data.posts.map((p) => p.day))].sort().reverse();
  const shown = shownPosts(a);
  const selected = data.posts.filter((p) => p.status === 'selected').length;
  return [
    ui.row([
      ui.choice('fAccount', accounts, a.ui.filter.account),
      ui.choice('fDay', [{ value: 'all', label: t('filter.days') }, ...days.map((d) => ({ value: d, label: d }))], a.ui.filter.day),
    ], { gap: 'small', wrap: true }),
    ui.row([
      ui.choice('fStatus', ['all', 'refined', 'selected', 'skipped', 'drafted', 'uploaded'].map((s) => ({ value: s, label: s === 'all' ? t('filter.stage') : t(`stage.${s}`) })), a.ui.filter.status),
      ui.choice('fKind', ['all', 'post', 'quote', 'reply', 'repost'].map((k) => ({ value: k, label: k === 'all' ? t('filter.kind') : t(`kind.${k}`) })), a.ui.filter.kind),
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
      ui.choice('style', data.styles.map((s) => ({ value: s.id, label: s.name })), a.ui.styleId),
      ui.button('makeDrafts', t('convert.make', { n: selected }), { icon: 'wand-sparkles', variant: 'primary', disabled: !selected }),
    ]),
  ];
}

function renderDrafts(a) {
  const open = data.drafts.find((d) => d.id === a.ui.openDraft);
  const list = data.drafts.filter((d) => (a.ui.draftFilter === 'open' ? d.status === 'draft' || d.status === 'ready' : d.status === a.ui.draftFilter));
  const items = [
    ui.choice('draftFilter', [{ value: 'open', label: t('drafts.todo') }, ...DRAFT_STATUS.map((s) => ({ value: s, label: t(`stage.${s}`) }))], a.ui.draftFilter),
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
    const check = checkDraft({ ...open, text: a.ui.draftText });
    items.push(
      ui.section(t('draft.heading', { style: open.styleName, status: t(`stage.${open.status}`) }), [
        ui.input('draftText', { value: a.ui.draftText, rows: 8, placeholder: t('draft.placeholder') }),
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

function renderStyles(a) {
  const style = data.styles.find((s) => s.id === a.ui.editStyle) ?? data.styles[0];
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
    ui.row([ui.choice('editStyle', data.styles.map((s) => ({ value: s.id, label: s.name })), style.id), ui.button('newStyle', t('style.new'), { icon: 'plus' })], { gap: 'small', wrap: true }),
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

function render(instance) {
  const a = auto(instance);
  const views = { automation: renderAutomation, posts: renderPosts, drafts: renderDrafts, styles: renderStyles };
  const ready = data.drafts.filter((d) => d.status === 'ready').length;
  const tree = ui.column([
    ui.choice('view', [
      { value: 'automation', label: t('view.automation') },
      { value: 'posts', label: t('view.posts', { n: data.posts.filter((p) => p.status === 'refined' || p.status === 'selected').length }) },
      { value: 'drafts', label: t('view.drafts', { n: ready }) },
      { value: 'styles', label: t('view.styles') },
    ], a.ui.view),
    ui.divider(),
    ...(views[a.ui.view] ?? renderAutomation)(a),
  ]);
  return plugin.setPanel(tree, instance ? { instance } : {});
}

async function renderAll() {
  await Promise.all([...autos.keys()].map((instance) => render(instance)));
}

// ---------------------------------------------------------------------------------------------
// Events: every one says which automation's panel it came from
// ---------------------------------------------------------------------------------------------

/** `handler(a, event)` for element `id`, with the automation the event belongs to. */
function on(id, handler) {
  plugin.onEvent(id, async (event) => {
    const a = auto(event.instance ?? MAIN);
    await handler(a, event);
  });
}

async function reveal(a, path) {
  await plugin.revealPath(path).catch((err) => note(a, err.message));
}

function editStyle(a, change) {
  data.styles = data.styles.map((s) => (s.id === a.ui.editStyle ? { ...s, ...change } : s));
}

on('view', (a, e) => ((a.ui.view = e.value), render(a.instance)));
on('title', async (a, e) => {
  a.config.title = (e.value ?? '').trim() || null;
  await saveConfig(a);
});
on('profile', async (a, e) => {
  if (a.busy || e.value === a.config.profile) return render(a.instance);
  a.config.profile = e.value;
  await saveConfig(a);
  // The page moves to the profile's sign-in.
  if (a.tabId !== null) await plugin.browser.close(a.tabId).catch(() => {});
  a.tabId = null;
  a.window = {};
  await refreshSignIn(a.config.profile);
  await engagementOf(a.config.profile);
  await ensurePage(a).catch((err) => note(a, err.message));
  await render(a.instance);
});
on('newProfile', async (a, e) => {
  a.ui.newProfile = e.value ?? '';
  if (e.event === 'submit') await addProfile(a);
});
on('addProfile', (a) => addProfile(a));
async function addProfile(a) {
  const name = a.ui.newProfile.trim();
  if (!/^[a-z0-9_-]{1,32}$/.test(name) || name === 'default') {
    a.status = t('auto.profile.invalid');
    return render(a.instance);
  }
  a.ui.newProfile = '';
  if (!data.profiles.includes(name)) data.profiles = [...data.profiles, name];
  a.config.profile = name;
  await saveConfig(a);
  if (a.tabId !== null) await plugin.browser.close(a.tabId).catch(() => {});
  a.tabId = null;
  // The store is made the first time a page opens in it: signed out, until the user signs in.
  await ensurePage(a).catch((err) => note(a, err.message));
  await refreshProfiles();
  await refreshSignIn(name);
  await render(a.instance);
}
on('removeProfile', async (a) => {
  const name = a.config.profile;
  if (name === 'default' || a.busy) return;
  await plugin.browser.removeProfile(name).catch((err) => note(a, err.message));
  signIns.delete(name);
  // Every automation that used it goes back to the default sign-in.
  for (const other of autos.values()) {
    if (other.config.profile === name) {
      other.config.profile = 'default';
      other.tabId = null;
      await saveConfig(other);
      await ensurePage(other).catch(() => {});
    }
  }
  await refreshProfiles();
  await refreshSignIn('default');
  await renderAll();
});
on('signin', async (a) => {
  a.status = t('sign_in.waiting');
  await render(a.instance);
  const result = await plugin.browser.signIn('x.com', { message: t('sign_in.message'), profile: a.config.profile, instance: a.instance || undefined });
  a.status = result.signedIn ? t('sign_in.done') : t('sign_in.failed', { reason: result.reason ?? '?' });
  await refreshSignIn(a.config.profile);
  await renderAll();
});
on('newTarget', async (a, e) => {
  a.ui.newTarget = e.value ?? '';
  if (e.event === 'submit') await addTarget(a);
});
on('addTarget', (a) => addTarget(a));
async function addTarget(a) {
  const account = accountOf(profileUrl(a.ui.newTarget || ''));
  if (!account || !/^[A-Za-z0-9_]{1,15}$/.test(account)) {
    a.status = t('account.invalid');
    return render(a.instance);
  }
  a.ui.newTarget = '';
  if (!a.config.targets.includes(account)) a.config.targets = [...a.config.targets, account];
  await store.saveAccount(await store.account(SITE, account));
  await saveConfig(a);
  a.status = t('account.added', { account });
  await reload();
  await render(a.instance);
}
on('targets', async (a, e) => {
  if (e.event !== 'action') return;
  const account = e.item;
  if (e.action === 'remove') {
    a.config.targets = a.config.targets.filter((x) => x !== account);
    await saveConfig(a);
  }
  if (e.action === 'folder') await reveal(a, join(store.root, SITE));
  if (e.action === 'backfill' && !a.busy && !busyAccounts.has(account)) {
    a.busy = true;
    busyAccounts.add(account);
    try {
      const tabId = await ensurePage(a);
      await collect(plugin.browser, store, account, { limit: a.config.limit, tabId, profile: a.config.profile, instance: a.instance || undefined, backfill: true, engine: a.config.title || a.instance, log: (l) => plugin.log(l) });
    } finally {
      busyAccounts.delete(account);
      a.busy = false;
      await reload();
    }
  }
  await renderAll();
});
on('schedule', async (a, e) => {
  a.config.schedule = Number(e.value) || 0;
  await saveConfig(a);
  if (!a.busy) schedule(a);
  await render(a.instance);
});
on('limit', async (a, e) => {
  a.config.limit = Math.min(Number(e.value) || 10, MAX_PER_RUN);
  await saveConfig(a);
  await render(a.instance);
});
on('source', async (a, e) => {
  if (a.busy || !SOURCES.includes(e.value)) return render(a.instance);
  a.config.source = e.value;
  await saveConfig(a);
  await render(a.instance);
});
on('newKeyword', async (a, e) => {
  a.ui.newKeyword = e.value ?? '';
  if (e.event === 'submit') await addKeyword(a);
});
on('addKeyword', (a) => addKeyword(a));
async function addKeyword(a) {
  const keyword = a.ui.newKeyword.replace(/\s+/g, ' ').trim().slice(0, 100);
  if (!keyword) return;
  a.ui.newKeyword = '';
  if (!a.config.keywords.includes(keyword)) a.config.keywords = [...a.config.keywords, keyword];
  await saveConfig(a);
  await render(a.instance);
}
on('keywords', async (a, e) => {
  if (e.event !== 'action' || e.action !== 'remove') return;
  a.config.keywords = a.config.keywords.filter((k) => k !== e.item);
  await saveConfig(a);
  await render(a.instance);
});
/** A setting that changes the panel: saved, then drawn again. */
const setting = (id, apply) =>
  on(id, async (a, e) => {
    apply(a.config, e.value);
    await saveConfig(a);
    await render(a.instance);
  });
/** A text setting: saved as it is typed; the field keeps its own text meanwhile. */
const textSetting = (id, apply) =>
  on(id, async (a, e) => {
    apply(a.config, e.value ?? '');
    await saveConfig(a);
  });
const ms = (value, fallback) => {
  const n = Math.round(Number(String(value).trim()));
  return Number.isFinite(n) && String(value).trim() !== '' ? Math.min(Math.max(n, 0), 600000) : fallback;
};
setting('minLikes', (c, v) => (c.minLikes = Number(v) || 0));
setting('maxAge', (c, v) => (c.maxAgeHours = Number(v) || 0));
setting('tCollect', (c, v) => (c.tasks.collect = !!v));
setting('tLike', (c, v) => (c.tasks.like = !!v));
setting('tReply', (c, v) => (c.tasks.reply = !!v));
setting('rAi', (c, v) => (c.reply.ai = !!v));
textSetting('rPattern', (c, v) => (c.reply.pattern = v.slice(0, 1000)));
textSetting('rInstructions', (c, v) => (c.reply.instructions = v.slice(0, 2000)));
textSetting('rRequired', (c, v) => (c.reply.required = v.slice(0, 1000)));
textSetting('paceMin', (c, v) => (c.pace.minMs = ms(v, c.pace.minMs)));
textSetting('paceMax', (c, v) => (c.pace.maxMs = ms(v, c.pace.maxMs)));
setting('perWindow', (c, v) => (c.pace.maxPer10Min = Math.max(1, Number(v) || 1)));
setting('likesDay', (c, v) => (c.pace.likesPerDay = Number(v) || LIKES_A_DAY[0]));
setting('repliesDay', (c, v) => (c.pace.repliesPerDay = Number(v) || REPLIES_A_DAY[0]));
setting('perUnit', (c, v) => (c.pace.perUnit = Number(v) || 1));
on('run', (a) => runAutomation(a.instance));
on('stop', (a) => {
  a.stop = true;
  return render(a.instance);
});

on('fAccount', (a, e) => ((a.ui.filter.account = e.value), render(a.instance)));
on('fDay', (a, e) => ((a.ui.filter.day = e.value), render(a.instance)));
on('fStatus', (a, e) => ((a.ui.filter.status = e.value), render(a.instance)));
on('fKind', (a, e) => ((a.ui.filter.kind = e.value), render(a.instance)));
on('selectTop', async (a) => {
  const picks = shownPosts(a)
    .filter((post) => post.status === 'refined' && post.kind !== 'repost' && !post.pinned)
    .sort((x, y) => (y.metrics.likes ?? 0) - (x.metrics.likes ?? 0))
    .slice(0, 3);
  await setStatus(picks.map((p) => p.id), 'selected');
  a.status = t('select.done', { n: picks.length });
  await renderAll();
});
on('selectShown', async (a) => {
  await setStatus(shownPosts(a).filter((p) => p.status === 'refined').map((p) => p.id), 'selected');
  await renderAll();
});
on('clearSelection', async () => {
  await setStatus(data.posts.filter((p) => p.status === 'selected').map((p) => p.id), 'refined');
  await renderAll();
});
on('posts', async (a, e) => {
  const post = data.posts.find((p) => p.id === e.item);
  if (!post) return;
  if (e.event === 'action' && e.action === 'select') await setStatus([post.id], 'selected');
  if (e.event === 'action' && e.action === 'unselect') await setStatus([post.id], 'refined');
  if (e.event === 'action' && e.action === 'skip') await setStatus([post.id], 'skipped');
  if (e.event === 'action' && e.action === 'folder') await reveal(a, join(store.postDir(post.site, post.day, post.account, post.id), 'post.md'));
  if (e.event === 'select') await setStatus([post.id], post.status === 'selected' ? 'refined' : 'selected');
  await renderAll();
});
on('style', (a, e) => ((a.ui.styleId = e.value), render(a.instance)));
on('makeDrafts', async (a) => {
  await makeDrafts(a);
  await renderAll();
});
on('draftFilter', (a, e) => ((a.ui.draftFilter = e.value), render(a.instance)));
on('drafts', async (a, e) => {
  const draft = data.drafts.find((d) => d.id === e.item);
  if (!draft) return;
  if (e.action === 'folder') await reveal(a, join(store.draftDir(draft.id), 'draft.md'));
  else {
    a.ui.openDraft = draft.id;
    a.ui.draftText = draft.text;
  }
  await render(a.instance);
});
on('draftText', (a, e) => {
  a.ui.draftText = e.value ?? '';
});
on('saveDraft', async (a) => {
  await updateDraft(a.ui.openDraft, { text: a.ui.draftText });
  a.status = t('draft.saved');
  await renderAll();
});
on('ready', async (a) => {
  await updateDraft(a.ui.openDraft, { text: a.ui.draftText, status: 'ready' });
  await renderAll();
});
on('unready', async (a) => {
  await updateDraft(a.ui.openDraft, { status: 'draft' });
  await renderAll();
});
on('discard', async (a) => {
  await updateDraft(a.ui.openDraft, { status: 'discarded' });
  a.ui.openDraft = null;
  await renderAll();
});
on('closeDraft', (a) => ((a.ui.openDraft = null), render(a.instance)));
on('editStyle', (a, e) => ((a.ui.editStyle = e.value), render(a.instance)));
on('newStyle', (a) => {
  const id = `style-${Date.now().toString(36)}`;
  data.styles = [...data.styles, { ...DEFAULT_STYLES[0], id, name: t('style.new') }];
  a.ui.editStyle = id;
  return render(a.instance);
});
on('sName', (a, e) => editStyle(a, { name: e.value }));
on('sKind', (a, e) => (editStyle(a, { kind: e.value }), render(a.instance)));
on('sPerPost', (a, e) => (editStyle(a, { perPost: !!e.value }), render(a.instance)));
on('sTemplate', (a, e) => editStyle(a, { template: e.value }));
on('sInstructions', (a, e) => editStyle(a, { instructions: e.value }));
on('sHeader', (a, e) => editStyle(a, { header: e.value }));
on('sItem', (a, e) => editStyle(a, { item: e.value }));
on('sFooter', (a, e) => editStyle(a, { footer: e.value }));
on('sMedia', (a, e) => (editStyle(a, { media: e.value }), render(a.instance)));
on('sHashtags', (a, e) => (editStyle(a, { hashtags: e.value }), render(a.instance)));
on('sMax', (a, e) => (editStyle(a, { maxLength: Number(e.value) }), render(a.instance)));
on('saveStyle', async (a) => {
  await store.saveStyles(data.styles);
  a.status = t('style.saved');
  await renderAll();
});
on('deleteStyle', async (a) => {
  if (data.styles.length <= 1) return;
  data.styles = data.styles.filter((s) => s.id !== a.ui.editStyle);
  a.ui.editStyle = data.styles[0].id;
  await store.saveStyles(data.styles);
  await renderAll();
});

plugin
  .onActivate(async (info) => {
    setLanguage(info.language);
    store = createStore(info.plugin.dataDir);
    await reload();
    await refreshProfiles();
    watchSignIns();
  })
  // Agentty names every automation there is when the plugin starts, and each new one after.
  .onInstanceOpen(({ instance, title }) => openAutomation(instance, title))
  .onInstanceClose(({ instance }) => closeAutomation(instance))
  .onPanelOpen(async () => {
    await reload();
    // Outside a workspace (the user chose another mode) the panel is one automation of its own.
    const instances = await plugin.instances().catch(() => []);
    if (!instances.length) await openAutomation(MAIN, null);
    await renderAll();
  })
  .onBrowserHidden(() => {})
  .start();
