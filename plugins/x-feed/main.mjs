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
import { composeInPage, composeState, discardCompose, fingerprint, mediaForPage, pressPost, sameText } from './lib/publish.mjs';
import { createStore, day, DRAFT_STATUS, readJson, writeJson, writeText } from './lib/store.mjs';
import { accountOf, appReady, goToPath, markPinned, profileUrl, readTimeline, scrollLikeAHand } from './lib/x.mjs';
import { setLanguage, t } from './lib/i18n.mjs';

const plugin = createPlugin();
const SITE = 'x';
const HOME = 'https://x.com/home';
/** The key of the panel outside a workspace (the user put the plugin in another mode). */
const MAIN = '';
const SCHEDULES = [0, 30, 60, 180, 1440];
const SOURCES = ['accounts', 'top', 'latest'];
const MIN_LIKES = [0, 10, 100, 1000];
const MAX_AGES = [24, 72, 0];
/** How fast likes and replies go, in three words; "custom" once the details were changed. */
const SPEEDS = {
  safe: { minMs: 2000, maxMs: 5000, maxPer10Min: 2, likesPerDay: 30, repliesPerDay: 10 },
  normal: { minMs: 100, maxMs: 5000, maxPer10Min: 3, likesPerDay: 50, repliesPerDay: 20 },
  fast: { minMs: 100, maxMs: 3000, maxPer10Min: 6, likesPerDay: 100, repliesPerDay: 40 },
};
/** The steps of setting up an automation, in order. */
const STEPS = ['account', 'source', 'actions', 'timing'];
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
/** Which X account each sign-in profile is: `{ profile: '@handle' }`. */
let handles = {};

/** An automation's settings as first made; saved ones are laid over it. */
const defaultConfig = () => ({
  title: null,
  profile: 'default',
  targets: [],
  limit: 10,
  schedule: 60,
  // Runs on its own every `schedule` minutes once the user turned it on.
  enabled: false,
  // Which posts: the accounts' own, or the top or latest posts found for keywords.
  source: 'accounts',
  keywords: [],
  minLikes: 0,
  maxAgeHours: 24,
  tasks: { collect: true, like: false, reply: false },
  reply: { pattern: '', ai: false, instructions: '', required: '', maxLength: 280 },
  // A random pause of minMs..maxMs between actions, at most 1..maxPer10Min actions (drawn anew)
  // every 10 minutes, and daily caps per sign-in.
  pace: { preset: 'normal', ...SPEEDS.normal, perUnit: 3 },
});

function withDefaults(saved) {
  const base = defaultConfig();
  if (!saved) return base;
  return {
    ...base,
    ...saved,
    // Settings from before the on/off switch: an interval meant "on".
    enabled: saved.enabled ?? (saved.schedule ?? 0) > 0,
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
        // The step shown open; `undefined` until the user picks one: the first unfinished one.
        step: undefined,
        advanced: false,
        newTarget: '',
        newKeyword: '',
        newProfile: '',
        filter: { account: 'all', day: 'all', status: 'refined', kind: 'all' },
        styleId: DEFAULT_STYLES[0].id,
        draftFilter: 'open',
        openDraft: null,
        draftText: '',
        editStyle: DEFAULT_STYLES[0].id,
        logFilter: { who: 'all', action: 'all', failed: false },
      },
    };
    autos.set(instance, a);
  }
  return a;
}

/** The latest actions (newest first), as the log view shows them; all of them are on disk. */
const recentActions = [];
const RECENT_ACTIONS = 300;

/**
 * Records one action an automation took in X: when, which automation and sign-in, what, where
 * (URL), on what, whether it worked and the details. Kept in `actions/<day>.jsonl`.
 */
async function act(a, action, fields = {}) {
  const entry = {
    at: new Date().toISOString(),
    automation: a.config.title || a.instance || 'main',
    instance: a.instance || null,
    profile: a.config.profile,
    action,
    ok: true,
    ...fields,
  };
  recentActions.unshift(entry);
  recentActions.length = Math.min(recentActions.length, RECENT_ACTIONS);
  await store.appendAction(entry).catch((err) => plugin.log(`action log: ${err.message}`));
  if (a.ui.view === 'log') await render(a.instance);
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

/** What the user calls a sign-in: its @handle once known. */
const accountLabel = (profile) => handles[profile] ?? (profile === 'default' ? t('acct.default') : t('acct.other'));

/** Reads which X account the automation's page is signed in to, and remembers it. */
async function learnHandle(a) {
  if (a.tabId === null || !signedIn(a.config.profile)) return;
  const handle = await plugin.browser.eval(a.tabId, whoAmI).catch(() => null);
  if (!handle || handles[a.config.profile] === handle) return;
  handles = { ...handles, [a.config.profile]: handle };
  await store.saveHandles(handles);
  await renderAll();
}

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
    if (changed) {
      for (const a of autos.values()) await learnHandle(a);
      await renderAll();
    }
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
  // Untitled: the name its tab shows ("Automation 2"), so the log and the tab say the same.
  if (!a.config.title && instance) {
    const list = await plugin.instances().catch(() => []);
    const index = list.findIndex((i) => i.instance === instance);
    a.config.title = t('auto.default_name', { n: index >= 0 ? index + 1 : list.length + 1 });
    await saveConfig(a);
  }
  // The tab shows the automation's own name, whatever it last showed.
  if (instance && a.config.title && a.config.title !== title) await plugin.setInstanceTitle(instance, a.config.title).catch(() => {});
  await refreshSignIn(a.config.profile);
  await engagementOf(a.config.profile);
  try {
    await ensurePage(a);
  } catch (err) {
    note(a, err.message);
  }
  schedule(a);
  await render(instance);
  // Once the page is up: whose account it is.
  setTimeout(() => learnHandle(a).catch(() => {}), 8000);
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
  if (!minutes || !a.config.enabled) return;
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
  const tasks = [collecting(c) && 'collect', c.tasks.like && 'like', c.tasks.reply && 'reply'].filter(Boolean);
  await act(a, 'run', { target: units.map((u) => unitName(c, u)).join(', '), detail: `${c.source} · ${tasks.join('+')}` });
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
    await act(a, 'done', { ok: !a.stop, detail: a.stop ? t('auto.stop') : a.status });
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
async function goTo(a, tabId, path) {
  const ready = await plugin.browser.eval(tabId, appReady).catch(() => false);
  const moved = ready && (await plugin.browser.eval(tabId, goToPath, { path }).catch(() => false));
  if (!moved) {
    await plugin.browser.navigate(tabId, HOME);
    await plugin.browser.wait(tabId, { timeoutMs: 30000 });
    await sleep(1500 + Math.random() * 2000);
    await plugin.browser.eval(tabId, goToPath, { path });
  }
  await act(a, 'open', { url: `https://x.com${path}`, detail: moved ? '' : 'from home' });
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
      // An account the user listed is meant, their own included; a keyword never picks their posts.
      self: c.source === 'accounts' ? null : a.self,
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
  await goTo(a, tabId, sourcePath(c.source, unit));
  if (!a.self) a.self = await plugin.browser.eval(tabId, whoAmI).catch(() => null);
  if (a.self && handles[c.profile] !== a.self) {
    handles = { ...handles, [c.profile]: a.self };
    await store.saveHandles(handles);
  }
  const { signin, posts } = await findPosts(a, unit, tabId, state);
  if (signin) {
    a.status = t('sign_in.asked');
    await act(a, 'signin', { ok: false, target: unitName(c, unit), detail: t('sign_in.asked') });
    result.signin = true;
    return result;
  }
  await act(a, 'find', { target: unitName(c, unit), detail: posts.map((p) => p.url).join(' ') || t('eng.none') });
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
    await act(a, 'agent', { detail: `${posts.length} → ${dir}` });
  } catch (err) {
    note(a, t('eng.agent_failed', { reason: err.message }));
    await act(a, 'agent', { ok: false, detail: err.message });
    return new Map();
  }
  const deadline = Date.now() + 5 * 60 * 1000;
  const ids = new Set(posts.map((p) => p.id));
  while (Date.now() < deadline && !a.stop) {
    await sleep(3000);
    const list = await readJson(join(dir, 'replies.json'), null);
    if (!Array.isArray(list)) continue;
    const written = new Map(list.filter((e) => e && ids.has(String(e.id)) && typeof e.text === 'string').map((e) => [String(e.id), e.text.trim()]));
    await act(a, 'agent', { detail: `${written.size}/${posts.length} written` });
    return written;
  }
  note(a, t('eng.agent_failed', { reason: a.stop ? t('auto.stop') : 'timeout' }));
  await act(a, 'agent', { ok: false, detail: a.stop ? 'stopped' : 'timeout' });
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
    await act(a, 'wait', { detail: `10 min cap ${a.window.cap}, until ${new Date(Date.now() + wait).toISOString()}` });
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
  const record = async (action, ok, extra = {}) => {
    state.log.push({ at: Date.now(), day: day(), action, ok, id: post.id, handle: post.handle, url: post.url, automation: c.title || a.instance, ...extra });
    note(a, ok ? t(action === 'like' ? 'eng.liked' : 'eng.replied', { handle: who }) : t('eng.failed', { action: t(`act.${action}`), handle: who, error: extra.error ?? '?' }));
    await act(a, action, { ok, url: post.url, target: post.handle, detail: extra.error ?? extra.text ?? '' });
  };
  const today = countToday(state.log);
  const likesLeft = today.like < c.pace.likesPerDay;
  const repliesLeft = today.reply < c.pace.repliesPerDay;
  const wantLike = c.tasks.like && likesLeft;
  const wantReply = c.tasks.reply && repliesLeft && reply?.ok;
  if ((!c.tasks.like || !likesLeft) && (!c.tasks.reply || !repliesLeft)) {
    note(a, t('eng.daily', { action: [c.tasks.like && t('act.like'), c.tasks.reply && t('act.reply')].filter(Boolean).join(', ') }));
    await act(a, 'limit', { ok: false, url: post.url, detail: `today ${today.like} likes, ${today.reply} replies` });
    done.capped = true;
    return done;
  }
  if (!wantLike && !wantReply) return done;
  await goTo(a, tabId, new URL(post.url).pathname);
  const engaged = { at: Date.now() };
  if (wantLike && (await paceWait(a, state))) {
    const r = await plugin.browser.eval(tabId, likeInPage, { id: post.id }, { timeoutMs: 20000 }).catch((err) => ({ error: err.message }));
    if (r.liked) {
      done.like = 1;
      engaged.like = true;
      await record('like', true);
    } else if (r.already) {
      engaged.like = true;
      await act(a, 'like', { url: post.url, target: post.handle, detail: 'already liked' });
    } else {
      await record('like', false, { error: r.error ?? (r.found ? 'no like button' : 'post not on the page') });
    }
  }
  if (c.tasks.reply && !reply?.ok) await act(a, 'reply', { ok: false, url: post.url, target: post.handle, detail: reply?.problems?.join(', ') ?? 'no reply' });
  if (wantReply && (await paceWait(a, state))) {
    const r = await plugin.browser.eval(tabId, replyInPage, { text: reply.text }, { timeoutMs: 30000 }).catch((err) => ({ error: err.message }));
    if (r.sent) {
      done.reply = 1;
      engaged.reply = reply.text;
      await record('reply', true, { text: reply.text });
    } else {
      await record('reply', false, { error: r.error });
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
    await act(a, 'signin', { ok: !!answer.signedIn, url: 'https://x.com', detail: answer.reason ?? '' });
    if (!answer.signedIn) {
      a.status = t('sign_in.failed', { reason: answer.reason ?? '?' });
      return run;
    }
    run = await collect(plugin.browser, store, account, options());
  }
  const failed = run.failed.length ? t('collect.failed', { n: run.failed.length }) : '';
  const gap = run.gap ? t('collect.gap') : '';
  note(a, t('collect.log', { account, new: run.new, seen: run.duplicates, failed, why: t(`why.${run.stoppedBecause}`), gap }));
  await act(a, 'collect', {
    ok: run.stoppedBecause !== 'error',
    url: profileUrl(account),
    target: `@${account}`,
    detail: `${run.new} new, ${run.duplicates} known, ${run.failed.length} failed, ${run.stoppedBecause}${run.error ? `: ${run.error}` : ''}`,
    posts: run.postIds,
  });
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

/**
 * Posts a ready draft from this automation's page (its sign-in): the compose window, the text,
 * the media; then the new post's address from the user's own timeline. The draft is `uploaded`
 * with that address, and so are the posts it was made from.
 */
async function publishDraft(a, id) {
  // Taken before anything is awaited: a second click must not post the draft twice.
  if (a.busy) return;
  a.busy = true;
  const draft = await store.draft(id).catch(() => null);
  if (!draft || draft.status !== 'ready') {
    a.busy = false;
    a.status = draft ? t('publish.not_ready') : '';
    return render(a.instance);
  }
  a.status = t('publish.posting');
  await render(a.instance);
  const fail = async (step, error) => {
    a.status = t('publish.failed', { error });
    await act(a, 'post', { ok: false, detail: `${step}: ${error}`, draft: id });
  };
  try {
    const files = await mediaForPage(draft.media.map((m) => m.path), store.draftDir(id));
    const tabId = await ensurePage(a);
    await goTo(a, tabId, '/home');
    const self = await plugin.browser.eval(tabId, whoAmI).catch(() => null);
    const composed = await plugin.browser.eval(tabId, composeInPage, { text: draft.text, files }, { timeoutMs: 60000 });
    if (!composed.ok) return await fail(composed.step, composed.error);
    // Uploads take their time: the window is looked at until everything is in and Post is on.
    let state = null;
    for (const deadline = Date.now() + 180000; Date.now() < deadline; ) {
      await sleep(1500);
      state = await plugin.browser.eval(tabId, composeState);
      if (!state.open || (state.canPost && !state.uploading && state.media >= files.length)) break;
    }
    if (!state?.open) return await fail('compose', 'the compose window closed');
    if (!state.canPost || state.uploading || state.media < files.length) {
      await plugin.browser.eval(tabId, discardCompose).catch(() => {});
      return await fail('media', state.message ?? 'the media did not finish uploading');
    }
    if (!sameText(state.text, draft.text)) {
      await plugin.browser.eval(tabId, discardCompose).catch(() => {});
      return await fail('text', `the window holds other text: ${state.text.slice(0, 120)}`);
    }
    await sleep(800 + Math.random() * 1200);
    const pressed = await plugin.browser.eval(tabId, pressPost);
    if (!pressed.pressed) return await fail('post', 'the Post button was off');
    for (const deadline = Date.now() + 30000; Date.now() < deadline; ) {
      await sleep(1000);
      state = await plugin.browser.eval(tabId, composeState);
      if (!state.open) break;
    }
    if (state.open) return await fail('post', state.message ?? 'the compose window did not close');

    // The new post, on the user's own timeline.
    let url = null;
    if (self) {
      await sleep(2500 + Math.random() * 2000);
      await goTo(a, tabId, `/${self.replace(/^@/, '')}`);
      const mark = fingerprint(draft.text).slice(0, 20);
      const page = await plugin.browser.eval(tabId, readTimeline, { seen: [] }, { timeoutMs: 20000 }).catch(() => ({ posts: [] }));
      const mine = page.posts.find((p) => sameHandle(p.handle, self) && (!mark || p.text.replace(/\s+/g, ' ').includes(mark)));
      url = mine?.url ?? null;
    }
    await updateDraft(id, { status: 'uploaded', postedUrl: url, postedAt: new Date().toISOString(), postedBy: self });
    for (const source of draft.sources) {
      const post = await store.post(source.site, source.day, source.account, source.id);
      if (post) await store.setPostStatus(post, 'uploaded');
    }
    await act(a, 'post', { url, target: self, detail: `${draft.text.replace(/\s+/g, ' ').slice(0, 120)} · ${files.length} media`, draft: id });
    a.status = t('publish.done', { url: url ?? t('publish.url_unknown') });
  } catch (err) {
    await fail('error', err.message);
    plugin.log(err.stack ?? String(err));
  } finally {
    a.busy = false;
    await reload();
    await renderAll();
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

// --- The automation, step by step ---------------------------------------------------------------

const scheduleLabel = (n) => (!n ? t('when.once') : n < 60 ? t('when.minutes', { n }) : n < 1440 ? t('when.hours', { n: n / 60 }) : t('when.daily'));
const likesLabel = (n) => (n ? t('src.min_likes', { n: n.toLocaleString() }) : t('src.any_likes'));
const ageLabel = (n) => (!n ? t('src.any_age') : n > 24 && n % 24 === 0 ? t('src.max_days', { n: n / 24 }) : t('src.max_age', { n }));

/** What the automation looks at, in a few words. */
function sourceSummary(c) {
  const units = unitsOf(c);
  if (!units.length) return t('step.source.none');
  const shown = units.slice(0, 3).map((u) => (c.source === 'accounts' ? `@${u}` : `"${u}"`)).join(', ') + (units.length > 3 ? ` +${units.length - 3}` : '');
  const kind = c.source === 'accounts' ? t('src.new_posts') : c.source === 'top' ? t('src.top') : t('src.latest');
  const filters = [c.minLikes ? likesLabel(c.minLikes) : null, c.maxAgeHours ? ageLabel(c.maxAgeHours) : null].filter(Boolean);
  return [`${shown} ${kind}`, ...filters].join(' · ');
}

function actionsSummary(c) {
  const parts = [collecting(c) && t('act.collect_short'), c.tasks.like && t('act.like'), c.tasks.reply && t('act.reply')].filter(Boolean);
  return parts.length ? parts.join(' + ') : t('step.actions.none');
}

const speedOf = (c) => (c.pace.preset && c.pace.preset !== 'custom' ? t(`speed.${c.pace.preset}`) : t('speed.custom'));

/** Each step: done or not, and what it is set to. */
function stepsOf(a) {
  const c = a.config;
  const replyReady = !c.tasks.reply || c.reply.ai || c.reply.pattern.trim() || c.reply.required.trim();
  return {
    account: { done: signedIn(c.profile), summary: signedIn(c.profile) ? accountLabel(c.profile) : t('step.account.todo') },
    source: { done: unitsOf(c).length > 0, summary: sourceSummary(c) },
    actions: { done: (collecting(c) || engaging(c)) && !!replyReady, summary: actionsSummary(c) },
    timing: { done: true, summary: engaging(c) ? `${scheduleLabel(c.schedule)} · ${speedOf(c)}` : scheduleLabel(c.schedule) },
  };
}

/** The step shown open: the one the user picked, else the first one not done (none when all are). */
function openStep(a) {
  // Picked, or being filled in (see `STEP_OF`): it stays open until Next, even once it is done.
  if (a.ui.step) return a.ui.step;
  const steps = stepsOf(a);
  // Otherwise the first unfinished step shows by itself, and none once all are done.
  return STEPS.find((id) => !steps[id].done) ?? null;
}

function renderAccountStep(a) {
  const c = a.config;
  const profiles = ['default', ...data.profiles];
  return [
    profiles.length > 1 ? ui.choice('profile', profiles.map((p) => ({ value: p, label: accountLabel(p) })), c.profile) : null,
    signedIn(c.profile)
      ? ui.row([ui.badge(t('acct.signed_in', { who: accountLabel(c.profile) }), 'success')], { gap: 'small' })
      : ui.column([ui.text(t('acct.sign_in_hint'), 'muted'), ui.button('signin', t('acct.sign_in'), { icon: 'key-round', variant: 'primary' })]),
    ui.row([
      data.profilesSupported ? ui.button('addAccount', t('acct.add'), { icon: 'plus', variant: 'ghost' }) : null,
      c.profile !== 'default' ? ui.button('removeProfile', t('acct.remove'), { variant: 'ghost' }) : null,
    ], { gap: 'small', wrap: true }),
    data.profilesSupported ? null : ui.text(t('auto.profile.unsupported'), 'small'),
  ];
}

function renderSourceStep(a) {
  const c = a.config;
  const byAccount = c.source === 'accounts';
  return [
    ui.choice('srcKind', [{ value: 'accounts', label: t('src.kind.accounts') }, { value: 'keywords', label: t('src.kind.keywords') }], byAccount ? 'accounts' : 'keywords'),
    ...(byAccount
      ? [
          ui.row([ui.input('newTarget', { placeholder: t('account.placeholder'), value: a.ui.newTarget }), ui.button('addTarget', t('account.add'), { icon: 'plus' })], { gap: 'small' }),
          ui.list(
            'targets',
            c.targets.map((account) => {
              const last = data.accounts.find((r) => r.account === account)?.lastRun;
              return {
                id: account,
                title: `@${account}`,
                subtitle: last ? t('account.checked', { when: new Date(last.at).toLocaleString() }) : t('account.never'),
                icon: 'x-twitter',
                tone: 'info',
                actions: [{ id: 'remove', icon: 'x', tooltip: t('auto.target.remove') }],
              };
            }),
            { empty: t('auto.targets.none') },
          ),
        ]
      : [
          ui.choice('srcSort', [{ value: 'top', label: t('src.top') }, { value: 'latest', label: t('src.latest') }], c.source),
          ui.row([ui.input('newKeyword', { placeholder: t('src.keyword_placeholder'), value: a.ui.newKeyword }), ui.button('addKeyword', t('account.add'), { icon: 'plus' })], { gap: 'small' }),
          ui.list('keywords', c.keywords.map((k) => ({ id: k, title: k, icon: 'search', tone: 'info', actions: [{ id: 'remove', icon: 'x', tooltip: t('auto.target.remove') }] })), { empty: t('src.keywords.none') }),
        ]),
    ui.text(t('src.filters'), 'small'),
    ui.choice('minLikes', MIN_LIKES.map((n) => ({ value: String(n), label: likesLabel(n) })), String(MIN_LIKES.includes(c.minLikes) ? c.minLikes : 0)),
    ui.choice('maxAge', MAX_AGES.map((n) => ({ value: String(n), label: ageLabel(n) })), String(MAX_AGES.includes(c.maxAgeHours) ? c.maxAgeHours : 0)),
  ];
}

function renderActionsStep(a) {
  const c = a.config;
  return [
    c.source === 'accounts' ? ui.toggle('tCollect', t('act.collect'), c.tasks.collect) : null,
    ui.toggle('tLike', t('act.like_long'), c.tasks.like),
    ui.toggle('tReply', t('act.reply_long'), c.tasks.reply),
    ...(c.tasks.reply
      ? [
          ui.input('rPattern', { value: c.reply.pattern, rows: 2, placeholder: t('reply.pattern_ph') }),
          ui.text(t('reply.fields'), 'small'),
          ui.toggle('rAi', t('reply.ai'), c.reply.ai),
          c.reply.ai ? ui.input('rInstructions', { value: c.reply.instructions, rows: 2, placeholder: t('reply.instructions') }) : null,
          ui.input('rRequired', { value: c.reply.required, rows: 2, placeholder: t('reply.required') }),
        ]
      : []),
    engaging(c) ? ui.text(t('act.warning'), 'error') : null,
  ];
}

function renderTimingStep(a) {
  const c = a.config;
  const p = c.pace;
  const speeds = [...Object.keys(SPEEDS), ...(p.preset === 'custom' ? ['custom'] : [])];
  const items = [ui.choice('schedule', SCHEDULES.map((n) => ({ value: String(n), label: scheduleLabel(n) })), String(SCHEDULES.includes(c.schedule) ? c.schedule : 60))];
  if (engaging(c)) {
    items.push(
      ui.choice('speed', speeds.map((s) => ({ value: s, label: t(`speed.${s}`) })), p.preset ?? 'custom'),
      ui.text(t('speed.desc', { min: p.minMs, max: p.maxMs, n: p.maxPer10Min, likes: p.likesPerDay, replies: p.repliesPerDay }), 'small'),
    );
  }
  items.push(ui.toggle('advanced', t('step.advanced'), a.ui.advanced));
  if (a.ui.advanced) {
    if (engaging(c)) {
      items.push(
        ui.text(t('safe.delay'), 'small'),
        ui.row([ui.input('paceMin', { value: String(p.minMs), placeholder: t('safe.min') }), ui.input('paceMax', { value: String(p.maxMs), placeholder: t('safe.max') })], { gap: 'small' }),
        ui.choice('perWindow', PER_WINDOW.map((n) => ({ value: String(n), label: t('safe.window', { n }) })), String(p.maxPer10Min)),
        c.tasks.like ? ui.choice('likesDay', LIKES_A_DAY.map((n) => ({ value: String(n), label: t('safe.likes_day', { n }) })), String(p.likesPerDay)) : null,
        c.tasks.reply ? ui.choice('repliesDay', REPLIES_A_DAY.map((n) => ({ value: String(n), label: t('safe.replies_day', { n }) })), String(p.repliesPerDay)) : null,
        ui.choice('perUnit', PER_UNIT.map((n) => ({ value: String(n), label: t('safe.per_unit', { n }) })), String(p.perUnit)),
      );
    }
    if (collecting(c)) items.push(ui.choice('limit', [5, 10, 20].filter((n) => n <= MAX_PER_RUN).map((n) => ({ value: String(n), label: t('per_run', { n }) })), String(c.limit)));
    items.push(ui.input('title', { placeholder: t('auto.name'), value: c.title ?? '' }));
  }
  return items;
}

const STEP_FORMS = { account: renderAccountStep, source: renderSourceStep, actions: renderActionsStep, timing: renderTimingStep };
const STEP_ICONS = { account: 'key-round', source: 'search', actions: 'heart', timing: 'clock' };

function renderAutomation(a) {
  const c = a.config;
  const steps = stepsOf(a);
  const open = openStep(a);
  const allDone = STEPS.every((id) => steps[id].done);
  const index = open ? STEPS.indexOf(open) : -1;
  const last = index === STEPS.length - 1;
  const items = [
    ui.text(t('flow.intro'), 'muted'),
    ui.list(
      'steps',
      STEPS.map((id, i) => ({
        id,
        title: `${i + 1}. ${t(`step.${id}`)}`,
        subtitle: steps[id].summary,
        icon: steps[id].done ? 'circle-check' : STEP_ICONS[id],
        tone: id === open ? 'info' : steps[id].done ? 'success' : 'neutral',
        actions: [{ id: 'edit', icon: id === open ? 'chevron-up' : 'pencil', tooltip: t('step.edit') }],
      })),
    ),
  ];
  if (open) {
    items.push(
      ui.section(`${index + 1}. ${t(`step.${open}`)}`, [
        ui.text(t(`step.${open}.help`), 'small'),
        ...STEP_FORMS[open](a),
        ui.row([
          ui.button('stepNext', last ? t('step.finish') : t('step.next'), { icon: 'chevron-right', variant: 'primary', disabled: !steps[open].done }),
        ]),
      ]),
    );
  }
  // What it will do, and the switch.
  if (allDone) {
    const sentence = t('flow.summary', { account: accountLabel(c.profile), source: sourceSummary(c), actions: actionsSummary(c) });
    items.push(
      ui.section(c.enabled ? t('flow.on') : t('flow.ready'), [
        ui.text(sentence),
        ui.text(c.enabled && c.schedule ? t('flow.every', { when: scheduleLabel(c.schedule) }) : c.schedule ? t('flow.off_hint', { when: scheduleLabel(c.schedule) }) : t('flow.once_hint'), 'muted'),
        ui.row([
          c.schedule
            ? c.enabled
              ? ui.button('disable', t('flow.turn_off'), { icon: 'circle-pause', variant: 'secondary' })
              : ui.button('enable', t('flow.turn_on'), { icon: 'play', variant: 'primary' })
            : null,
          a.busy
            ? ui.button('stop', a.stop ? t('auto.stopping') : t('auto.stop'), { icon: 'circle-x', variant: 'danger', disabled: a.stop })
            : ui.button('run', t('flow.run_once'), { icon: 'zap', variant: c.schedule ? 'ghost' : 'primary' }),
        ], { gap: 'small', wrap: true }),
      ]),
    );
  } else {
    items.push(ui.text(t('flow.not_ready'), 'muted'));
  }
  items.push(a.busy ? ui.spinner(a.status) : a.status ? ui.text(a.status, 'muted') : null);
  if (c.enabled && a.nextAt && !a.busy) items.push(ui.text(t('auto.next', { when: when(a.nextAt) }), 'small'));
  if (engaging(c)) {
    const state = engagements.get(c.profile);
    const today = state ? countToday(state.log) : { like: 0, reply: 0 };
    items.push(ui.text(t('eng.today', { likes: today.like, replies: today.reply }), 'small'));
  }
  if (a.log.length) {
    items.push(
      ui.section(t('collect.recent'), [...a.log.slice(0, 4).map((line) => ui.text(line, 'small')), ui.button('toLog', t('flow.all_log'), { icon: 'list', variant: 'ghost' })]),
    );
  }
  return items;
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
    ui.row([ui.button('openStyles', t('view.styles'), { icon: 'palette', variant: 'ghost' })]),
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
        open.postedAt ? ui.text(t('publish.posted_at', { when: new Date(open.postedAt).toLocaleString(), url: open.postedUrl ?? t('publish.url_unknown') }), 'small') : null,
        ui.row([
          ui.button('saveDraft', t('draft.save'), { icon: 'save' }),
          open.status === 'ready'
            ? ui.button('unready', t('draft.unready'), { variant: 'secondary' })
            : ui.button('ready', t('draft.ready'), { icon: 'circle-check', variant: 'primary', disabled: !check.ok }),
          open.status === 'ready' ? ui.button('publish', t('publish.now'), { icon: 'send', variant: 'primary', disabled: a.busy }) : null,
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
    ui.button('backToDrafts', t('style.back'), { icon: 'chevron-left', variant: 'ghost' }),
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

const ACTIONS = ['run', 'done', 'open', 'find', 'collect', 'like', 'reply', 'post', 'agent', 'wait', 'limit', 'signin'];
const ACTION_ICON = { run: 'play', done: 'circle-check', open: 'globe', find: 'search', collect: 'download', like: 'heart', reply: 'message-circle', post: 'send', agent: 'bot', wait: 'clock', limit: 'circle-x', signin: 'key-round' };

function shownActions(a) {
  const { who, action, failed } = a.ui.logFilter;
  return recentActions.filter(
    (e) =>
      (who === 'all' || (e.instance || 'main') === (a.instance || 'main')) &&
      (action === 'all' || e.action === action) &&
      (!failed || !e.ok),
  );
}

function renderLog(a) {
  const f = a.ui.logFilter;
  const shown = shownActions(a).slice(0, 150);
  return [
    ui.row([
      ui.choice('logWho', [{ value: 'all', label: t('log.all') }, { value: 'this', label: t('log.this') }], f.who),
      ui.choice('logAction', [{ value: 'all', label: t('log.any') }, ...ACTIONS.map((x) => ({ value: x, label: t(`log.a.${x}`) }))], f.action),
    ], { gap: 'small', wrap: true }),
    ui.row([ui.toggle('logFailed', t('log.failed_only'), f.failed), ui.button('logFolder', t('files'), { icon: 'folder', variant: 'ghost' })], { gap: 'small', wrap: true }),
    ui.text(t('log.count', { shown: shown.length, all: recentActions.length }), 'muted'),
    ui.list(
      'log',
      shown.map((e, index) => ({
        id: String(index),
        title: `${new Date(e.at).toLocaleString()} · ${t(`log.a.${e.action}`)}${e.target ? ` · ${e.target}` : ''}${e.ok ? '' : ` · ${t('log.failed')}`}`,
        subtitle: e.url || e.detail || '',
        detail: [e.automation, e.profile, e.url ? e.detail : ''].filter(Boolean).join(' · ').slice(0, 300),
        icon: ACTION_ICON[e.action] ?? 'circle',
        tone: e.ok ? (['like', 'reply', 'post'].includes(e.action) ? 'success' : 'neutral') : 'error',
        actions: e.url ? [{ id: 'open', icon: 'external-link', tooltip: t('log.open') }] : [],
      })),
      { empty: t('log.none') },
    ),
  ];
}

function render(instance) {
  const a = auto(instance);
  const views = { automation: renderAutomation, posts: renderPosts, drafts: renderDrafts, styles: renderStyles, log: renderLog };
  const ready = data.drafts.filter((d) => d.status === 'ready').length;
  const tree = ui.column([
    ui.choice('view', [
      { value: 'automation', label: t('view.automation') },
      { value: 'posts', label: t('view.posts', { n: data.posts.filter((p) => p.status === 'refined' || p.status === 'selected').length }) },
      { value: 'drafts', label: t('view.drafts', { n: ready }) },
      { value: 'log', label: t('view.log') },
    ], a.ui.view === 'styles' ? 'drafts' : a.ui.view),
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
    // Using a step's fields keeps that step open while it is filled in.
    if (STEP_OF[id] && a.ui.view === 'automation') a.ui.step = STEP_OF[id];
    await handler(a, event);
  });
}

/** Which step each field of the automation view belongs to. */
const STEP_OF = Object.fromEntries(
  Object.entries({
    account: ['profile', 'signin', 'addAccount', 'removeProfile'],
    source: ['srcKind', 'srcSort', 'newTarget', 'addTarget', 'targets', 'newKeyword', 'addKeyword', 'keywords', 'minLikes', 'maxAge'],
    actions: ['tCollect', 'tLike', 'tReply', 'rPattern', 'rAi', 'rInstructions', 'rRequired'],
    timing: ['schedule', 'speed', 'advanced', 'paceMin', 'paceMax', 'perWindow', 'likesDay', 'repliesDay', 'perUnit', 'limit', 'title'],
  }).flatMap(([step, ids]) => ids.map((id) => [id, step])),
);

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
on('signin', (a) => signInHere(a));
async function signInHere(a) {
  a.status = t('sign_in.waiting');
  await render(a.instance);
  const result = await plugin.browser.signIn('x.com', { message: t('sign_in.message'), profile: a.config.profile, instance: a.instance || undefined });
  await act(a, 'signin', { ok: !!result.signedIn, url: 'https://x.com', detail: result.reason ?? '' });
  a.status = result.signedIn ? t('sign_in.done') : t('sign_in.failed', { reason: result.reason ?? '?' });
  await refreshSignIn(a.config.profile);
  if (result.signedIn) setTimeout(() => learnHandle(a).catch(() => {}), 4000);
  await renderAll();
}
on('newTarget', async (a, e) => {
  a.ui.newTarget = e.value ?? '';
  if (e.event === 'submit') await addTarget(a);
});
on('addTarget', (a) => addTarget(a));
/**
 * Adds what was typed: one account or several (`@a, @b`, spaces or lines between them, profile
 * addresses too). What is not an account is left in the field, so it can be fixed.
 */
async function addTarget(a) {
  const pieces = String(a.ui.newTarget || '').split(/[\s,]+/).filter(Boolean);
  if (!pieces.length) {
    a.status = t('account.invalid');
    return render(a.instance);
  }
  const added = [];
  const wrong = [];
  for (const piece of pieces) {
    const account = accountOf(profileUrl(piece));
    if (!account || !/^[A-Za-z0-9_]{1,15}$/.test(account)) {
      wrong.push(piece);
      continue;
    }
    if (!a.config.targets.includes(account)) a.config.targets = [...a.config.targets, account];
    if (!added.includes(account)) added.push(account);
    await store.saveAccount(await store.account(SITE, account));
  }
  a.ui.newTarget = wrong.join(' ');
  await saveConfig(a);
  // What was added shows in the list; only what could not be added needs saying.
  a.status = wrong.length ? t('account.some_invalid', { wrong: wrong.join(', ') }) : '';
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
  if (!a.config.schedule) a.config.enabled = false;
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
/** Adds one keyword, or several separated by commas or lines (a keyword may have spaces). */
async function addKeyword(a) {
  const keywords = String(a.ui.newKeyword || '')
    .split(/[,\n]+/)
    .map((k) => k.replace(/\s+/g, ' ').trim().slice(0, 100))
    .filter(Boolean);
  if (!keywords.length) return;
  a.ui.newKeyword = '';
  a.config.keywords = [...new Set([...a.config.keywords, ...keywords])];
  await saveConfig(a);
  await render(a.instance);
}
on('keywords', async (a, e) => {
  if (e.event !== 'action' || e.action !== 'remove') return;
  a.config.keywords = a.config.keywords.filter((k) => k !== e.item);
  await saveConfig(a);
  await render(a.instance);
});
/** A switch's value: true, or the text "true" some senders use. */
const on_ = (v) => v === true || v === 'true';
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
setting('tCollect', (c, v) => (c.tasks.collect = on_(v)));
setting('tLike', (c, v) => (c.tasks.like = on_(v)));
setting('tReply', (c, v) => (c.tasks.reply = on_(v)));
setting('rAi', (c, v) => (c.reply.ai = on_(v)));
textSetting('rPattern', (c, v) => (c.reply.pattern = v.slice(0, 1000)));
textSetting('rInstructions', (c, v) => (c.reply.instructions = v.slice(0, 2000)));
textSetting('rRequired', (c, v) => (c.reply.required = v.slice(0, 1000)));
textSetting('paceMin', (c, v) => ((c.pace.minMs = ms(v, c.pace.minMs)), (c.pace.preset = 'custom')));
textSetting('paceMax', (c, v) => ((c.pace.maxMs = ms(v, c.pace.maxMs)), (c.pace.preset = 'custom')));
setting('perWindow', (c, v) => ((c.pace.maxPer10Min = Math.max(1, Number(v) || 1)), (c.pace.preset = 'custom')));
setting('likesDay', (c, v) => ((c.pace.likesPerDay = Number(v) || LIKES_A_DAY[0]), (c.pace.preset = 'custom')));
setting('repliesDay', (c, v) => ((c.pace.repliesPerDay = Number(v) || REPLIES_A_DAY[0]), (c.pace.preset = 'custom')));
setting('perUnit', (c, v) => (c.pace.perUnit = Number(v) || 1));
on('logWho', (a, e) => ((a.ui.logFilter.who = e.value), render(a.instance)));
on('logAction', (a, e) => ((a.ui.logFilter.action = e.value), render(a.instance)));
on('logFailed', (a, e) => ((a.ui.logFilter.failed = on_(e.value)), render(a.instance)));
on('logFolder', (a) => reveal(a, store.actionsDir));
on('log', async (a, e) => {
  const entry = shownActions(a).slice(0, 150)[Number(e.item)];
  if (!entry?.url || a.busy) return;
  // Shown in this automation's own page, as a click in X would.
  const tabId = await ensurePage(a).catch(() => null);
  if (tabId !== null) await goTo(a, tabId, new URL(entry.url).pathname).catch((err) => note(a, err.message));
});
on('steps', (a, e) => {
  if (!STEPS.includes(e.item)) return;
  a.ui.step = openStep(a) === e.item ? null : e.item;
  return render(a.instance);
});
on('stepNext', (a) => {
  // Step by step, in order; after the last one the steps fold away (an unfinished one reopens).
  const open = openStep(a);
  if (!open || !stepsOf(a)[open].done) return render(a.instance);
  a.ui.step = STEPS[STEPS.indexOf(open) + 1] ?? null;
  return render(a.instance);
});
on('srcKind', async (a, e) => {
  if (a.busy) return render(a.instance);
  a.config.source = e.value === 'accounts' ? 'accounts' : a.config.source === 'accounts' ? 'top' : a.config.source;
  await saveConfig(a);
  await render(a.instance);
});
on('srcSort', async (a, e) => {
  if (a.busy || !['top', 'latest'].includes(e.value)) return render(a.instance);
  a.config.source = e.value;
  await saveConfig(a);
  await render(a.instance);
});
on('speed', async (a, e) => {
  const preset = SPEEDS[e.value];
  if (!preset) return render(a.instance);
  a.config.pace = { ...a.config.pace, ...preset, preset: e.value };
  await saveConfig(a);
  await render(a.instance);
});
on('advanced', (a, e) => ((a.ui.advanced = on_(e.value)), render(a.instance)));
on('toLog', (a) => ((a.ui.view = 'log'), (a.ui.logFilter.who = 'this'), render(a.instance)));
on('enable', async (a) => {
  a.config.enabled = true;
  await saveConfig(a);
  // Turned on: a first run now, then on its own.
  if (a.busy) schedule(a);
  else await runAutomation(a.instance);
});
on('disable', async (a) => {
  a.config.enabled = false;
  await saveConfig(a);
  schedule(a);
  await render(a.instance);
});
on('addAccount', async (a) => {
  if (a.busy) return;
  // A sign-in of its own, named for the machine only: the user sees the @handle.
  await refreshProfiles();
  let n = 2;
  while (data.profiles.includes(`account-${n}`)) n += 1;
  a.ui.newProfile = `account-${n}`;
  await addProfile(a);
  await signInHere(a);
});
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
on('openStyles', (a) => ((a.ui.view = 'styles'), render(a.instance)));
on('backToDrafts', (a) => ((a.ui.view = 'drafts'), render(a.instance)));
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
on('publish', (a) => publishDraft(a, a.ui.openDraft));
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
on('sPerPost', (a, e) => (editStyle(a, { perPost: on_(e.value) }), render(a.instance)));
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
    handles = await store.handles();
    recentActions.push(...(await store.actions(RECENT_ACTIONS)));
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
