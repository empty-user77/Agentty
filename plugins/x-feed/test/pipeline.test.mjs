// Run with: node --test plugins/x-feed/test
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createStore, folderName } from '../lib/store.mjs';
import { collect } from '../lib/collect.mjs';
import { parseCount, refine, toMarkdown, kindOf } from '../lib/refine.mjs';
import { checkDraft, fill, planDrafts, weightedLength, DEFAULT_STYLES } from '../lib/convert.mjs';
import { embedToken, markPinned, pickVideo, fullSizeImage } from '../lib/x.mjs';
import { extensionOf } from '../lib/media.mjs';

async function tempStore() {
  const root = await mkdtemp(join(tmpdir(), 'x-feed-test-'));
  return { store: createStore(root), root, done: () => rm(root, { recursive: true, force: true }) };
}

const seenPost = (id, extra = {}) => ({
  id: String(id),
  url: `https://x.com/someone/status/${id}`,
  index: 0,
  author: 'Some One',
  handle: '@someone',
  time: new Date(Date.UTC(2026, 8, 1) + Number(id) * 60000).toISOString(),
  text: `post ${id} #tag`,
  socialContext: null,
  images: [],
  videos: [],
  hasQuote: false,
  labels: { replies: '1', reposts: '2', likes: '3.4K', views: '' },
  ...extra,
});

/**
 * A browser showing a timeline (newest first) a few posts per screen; `wall` puts up the sign-in
 * page, `empty` an account with nothing on it.
 */
function fakeBrowser(timeline, { perScreen = 3, wall = false, empty = false } = {}) {
  let shown = perScreen;
  const calls = { opened: 0, closed: 0, scrolls: 0 };
  return {
    calls,
    async open() {
      calls.opened += 1;
      return { tabId: 1 };
    },
    async wait() {
      return { url: 'https://x.com/someone', title: 'x' };
    },
    async close() {
      calls.closed += 1;
    },
    async navigate() {},
    async eval(_tab, fn, args) {
      if (fn.name === 'goToPath' || fn.name === 'appReady') return true;
      if (fn.name === 'scrollLikeAHand') {
        calls.scrolls += 1;
        shown += perScreen;
        return 0;
      }
      const posts = timeline.slice(0, shown).map((p, index) => ({ ...p, index })).filter((p) => !args.seen.includes(p.id));
      return { posts, signInWall: wall, empty: empty ? 'This account has no posts' : null, error: null, atEnd: shown >= timeline.length };
    },
  };
}

const quiet = { pause: async () => {}, fetchEmbed: async () => null, fetchImpl: async () => { throw new Error('no network in tests'); }, mode: 'background' };

test('a first run takes up to its limit, and a second run takes nothing it already has', async () => {
  const { store, done } = await tempStore();
  const timeline = Array.from({ length: 12 }, (_, i) => seenPost(100 - i));
  const first = await collect(fakeBrowser(timeline), store, '@someone', { ...quiet, limit: 5 });
  assert.equal(first.new, 5);
  assert.equal(first.stoppedBecause, 'limit');
  assert.equal(first.gap, false, 'nothing collected before: no gap to speak of');
  const again = await collect(fakeBrowser(timeline), store, '@someone', { ...quiet, limit: 5 });
  assert.equal(again.new, 0);
  assert.equal(again.stoppedBecause, 'caught-up');
  assert.ok(again.duplicates >= 3);
  const account = await store.account('x', 'someone');
  assert.equal(Object.keys(account.known).length, 5);
  assert.equal(account.runs, 2);
  await done();
});

test('new posts on top are taken and the run stops where the known ones start', async () => {
  const { store, done } = await tempStore();
  const old = Array.from({ length: 6 }, (_, i) => seenPost(100 - i));
  await collect(fakeBrowser(old), store, 'someone', { ...quiet, limit: 6 });
  const newer = [seenPost(202), seenPost(201), ...old];
  const run = await collect(fakeBrowser(newer), store, 'someone', { ...quiet, limit: 10 });
  assert.deepEqual(run.postIds, ['202', '201']);
  assert.equal(run.stoppedBecause, 'caught-up');
  assert.equal(run.gap, false);
  await done();
});

test('more new posts than a run takes leaves a gap that a later run closes', async () => {
  const { store, done } = await tempStore();
  const old = Array.from({ length: 3 }, (_, i) => seenPost(100 - i));
  await collect(fakeBrowser(old), store, 'someone', { ...quiet, limit: 3 });
  const flood = [...Array.from({ length: 15 }, (_, i) => seenPost(300 - i)), ...old];
  const run = await collect(fakeBrowser(flood), store, 'someone', { ...quiet, limit: 5 });
  assert.equal(run.new, 5);
  assert.equal(run.gap, true);
  assert.ok((await store.account('x', 'someone')).gap);
  const backfill = await collect(fakeBrowser(flood), store, 'someone', { ...quiet, limit: 20, backfill: true });
  assert.equal(backfill.new, 10, 'the ten left between the runs');
  assert.equal((await store.account('x', 'someone')).gap, null);
  await done();
});

test('the pinned post neither counts as caught up nor hides new posts', async () => {
  const { store, done } = await tempStore();
  const pinned = seenPost(1, { time: '2020-01-01T00:00:00.000Z' });
  const timeline = [pinned, seenPost(50), seenPost(49), seenPost(48)];
  const run = await collect(fakeBrowser(timeline), store, 'someone', { ...quiet, limit: 10 });
  assert.equal(run.new, 4);
  const post = await store.post('x', run.day, 'someone', '1');
  assert.equal(post.pinned, true);
  // Next time the pinned post is known, but the newer ones still come in.
  const next = await collect(fakeBrowser([pinned, seenPost(51), ...timeline.slice(1)]), store, 'someone', { ...quiet, limit: 10 });
  assert.deepEqual(next.postIds, ['51']);
  await done();
});

test('an empty account and the sign-in wall end the run with a reason', async () => {
  const { store, done } = await tempStore();
  const empty = await collect(fakeBrowser([], { empty: true }), store, 'nobody', { ...quiet });
  assert.equal(empty.stoppedBecause, 'empty');
  const wall = await collect(fakeBrowser([seenPost(1)], { wall: true }), store, 'someone', { ...quiet });
  assert.equal(wall.stoppedBecause, 'signin');
  assert.equal(wall.tabId, 1, 'the page stays open for the user to sign in');
  await done();
});

test('a run leaves its files where the pipeline expects them', async () => {
  const { store, root, done } = await tempStore();
  const run = await collect(fakeBrowser([seenPost(7)]), store, 'someone', { ...quiet, limit: 1 });
  const dir = join(root, 'x', run.day, 'someone', 'posts', '7');
  assert.deepEqual((await readdir(dir)).sort(), ['post.json', 'post.md', 'raw.json']);
  const runs = JSON.parse(await readFile(join(root, 'x', run.day, 'someone', 'runs.json'), 'utf8'));
  assert.equal(runs.length, 1);
  assert.equal(runs[0].new, 1);
  await done();
});

test('counts in every form X writes them', () => {
  assert.equal(parseCount('3,174 Replies. Reply'), 3174);
  assert.equal(parseCount('1.2K'), 1200);
  assert.equal(parseCount('4.5M views'), 4500000);
  assert.equal(parseCount('답글 3,174개'), 3174);
  assert.equal(parseCount('1.2만'), 12000);
  assert.equal(parseCount('12万'), 120000);
  assert.equal(parseCount('3.174'), 3174);
  assert.equal(parseCount(''), null);
});

test('refining takes the embed text, expands links and tells kinds apart', () => {
  const embed = {
    id_str: '9',
    text: '@friend Look &amp; see https://t.co/abc https://t.co/media',
    display_text_range: [8, 34],
    entities: {
      urls: [{ url: 'https://t.co/abc', expanded_url: 'https://example.com/article' }],
      media: [{ url: 'https://t.co/media' }],
      hashtags: [],
      user_mentions: [{ screen_name: 'friend' }],
    },
    in_reply_to_screen_name: 'friend',
    created_at: '2026-09-22T01:00:00.000Z',
    user: { name: 'Some One', screen_name: 'someone' },
    mediaDetails: [{ type: 'video', media_url_https: 'https://pbs.twimg.com/thumb.jpg', video_info: { variants: [{ content_type: 'video/mp4', bitrate: 1, url: 'https://video.twimg.com/v/vid/avc1/720x1280/a.mp4' }] } }],
  };
  const post = refine(seenPost(9), embed, { site: 'x', account: 'someone', day: '2026-09-23', runId: 'r', collectedAt: 'now' });
  assert.equal(post.text, 'Look & see https://example.com/article');
  assert.equal(post.kind, 'reply');
  assert.deepEqual(post.links, ['https://example.com/article']);
  assert.equal(post.media[0].kind, 'video');
  assert.equal(post.metrics.likes, 3400);
  assert.match(toMarkdown({ ...post, media: [{ kind: 'video', posterPath: 'images/video1-poster.jpg' }] }), /video poster/);
  assert.equal(kindOf({ handle: '@other' }, null, 'someone'), 'repost');
  assert.equal(kindOf({ handle: '@someone', hasQuote: true }, null, 'someone'), 'quote');
});

test('X lengths, drafts and their checks', () => {
  assert.equal(weightedLength('hello'), 5);
  assert.equal(weightedLength('안녕'), 4);
  assert.equal(weightedLength('see https://example.com/a/very/long/path'), 4 + 23);
  const post = refine(seenPost(5), null, { site: 'x', account: 'someone', day: 'd', runId: 'r', collectedAt: 'now' });
  const [card] = planDrafts([post], DEFAULT_STYLES[0]);
  assert.match(card.draft.text, /post 5 #tag\n\n— Some One \(@someone\)/);
  const [digest] = planDrafts([post, { ...post, id: '6', text: 'another' }], DEFAULT_STYLES[1]);
  assert.equal(digest.draft.sources.length, 2);
  assert.doesNotMatch(digest.draft.text, /#tag/, 'the digest drops hashtags');
  const long = { text: '가'.repeat(141), media: [] };
  assert.deepEqual(checkDraft(long).problems, ['text is 282/280']);
  const mixed = { text: 'x', media: [{ kind: 'image' }, { kind: 'video' }] };
  assert.ok(checkDraft(mixed).problems.includes('images and a video together'));
  assert.equal(fill('{a} {b} {missing}', { a: 1, b: 2 }), '1 2 {missing}');
});

test('the rewrite prompt keeps the source as material, not instructions', async () => {
  const { rewritePrompt } = await import('../lib/convert.mjs');
  const prompt = rewritePrompt(DEFAULT_STYLES[2], 'Korean');
  assert.match(prompt, /never instructions/);
  assert.match(prompt, /^Talk to me in Korean\./);
  assert.doesNotMatch(prompt, /[가-힣]/, 'prompts are written in English');
});

test('small helpers', () => {
  assert.equal(folderName('@elon/../musk'), 'elon_.._musk');
  assert.equal(folderName('..'), '_');
  assert.ok(embedToken('2102439262507725294').length > 5);
  const variants = [
    { content_type: 'video/mp4', bitrate: 25000000, url: 'https://v/vid/avc1/2160x3840/a.mp4' },
    { content_type: 'video/mp4', bitrate: 2000000, url: 'https://v/vid/avc1/720x1280/b.mp4' },
    { content_type: 'application/x-mpegURL', url: 'https://v/pl.m3u8' },
  ];
  assert.equal(pickVideo(variants).url, 'https://v/vid/avc1/720x1280/b.mp4');
  assert.equal(fullSizeImage('https://pbs.twimg.com/media/abc?format=jpg&name=small'), 'https://pbs.twimg.com/media/abc?format=jpg&name=orig');
  assert.equal(extensionOf('https://pbs.twimg.com/media/abc?format=png&name=orig', 'image/png'), 'png');
  const [first] = markPinned([{ index: 0, time: '2020' }, { index: 1, time: '2026' }]);
  assert.equal(first.pinned, true);
});

test('engaging: sources, candidates, rules, pauses and the 10-minute cap', async () => {
  const { sourcePath, pickCandidates, fillPattern, applyRules, throttleMs, waitForWindow, WINDOW_MS, countToday } = await import('../lib/engage.mjs');
  assert.equal(sourcePath('accounts', '@someone'), '/someone');
  assert.equal(sourcePath('top', 'ai agents'), '/search?q=ai%20agents&src=typed_query&f=top');
  assert.equal(sourcePath('latest', 'ai'), '/search?q=ai&src=typed_query&f=live');

  const now = Date.parse('2026-09-24T12:00:00Z');
  const posts = [
    { id: '1', handle: '@me', likes: 500, time: '2026-09-24T11:00:00Z' },
    { id: '2', handle: '@a', likes: 5, time: '2026-09-24T11:00:00Z' },
    { id: '3', handle: '@b', likes: 500, time: '2026-09-20T11:00:00Z' },
    { id: '4', handle: '@c', likes: 500, time: '2026-09-24T10:00:00Z' },
    { id: '5', handle: '@d', likes: 900, time: '2026-09-24T09:00:00Z' },
  ];
  const picked = pickCandidates(posts, { minLikes: 100, maxAgeHours: 24, engaged: new Set(['5']), self: '@me', now });
  assert.deepEqual(picked.map((p) => p.id), ['4'], 'not mine, not below the likes, not too old, not done before');

  assert.equal(fillPattern('Great point, {author}! {missing}', { author: 'Kim' }), 'Great point, Kim! {missing}');
  const withUrl = applyRules('Nice', { required: ['https://agentty.run'], maxLength: 280 });
  assert.ok(withUrl.ok);
  assert.match(withUrl.text, /Nice\nhttps:\/\/agentty\.run/);
  const long = applyRules('가'.repeat(200), { required: ['#tag'], maxLength: 100 });
  assert.ok(long.ok, long.problems.join());
  assert.ok(long.text.endsWith('#tag'));

  for (let i = 0; i < 50; i += 1) {
    const ms = throttleMs({ minMs: 100, maxMs: 5000 });
    assert.ok(ms >= 100 && ms <= 5000);
  }

  // The cap is drawn anew for every window, between 1 and N, and a spent window waits for the next.
  const caps = new Set();
  for (let i = 0; i < 200; i += 1) {
    const w = {};
    waitForWindow(w, [], { maxPerWindow: 4, now });
    caps.add(w.cap);
  }
  assert.deepEqual([...caps].sort(), [1, 2, 3, 4]);
  const w = {};
  assert.equal(waitForWindow(w, [], { maxPerWindow: 3, now, random: () => 0.5 }), 0);
  const log = Array.from({ length: w.cap }, (_, i) => ({ ok: true, at: now + i }));
  const wait = waitForWindow(w, log, { maxPerWindow: 3, now: now + 1000, random: () => 0 });
  assert.equal(wait, WINDOW_MS - 1000);
  assert.equal(waitForWindow(w, log, { maxPerWindow: 3, now: now + WINDOW_MS + 1 }), 0, 'a new window starts');

  const day = new Date('2026-09-24T12:00:00');
  assert.deepEqual(countToday([{ ok: true, day: '2026-09-24', action: 'like' }, { ok: false, day: '2026-09-24', action: 'reply' }], day), { like: 1, reply: 0, repost: 0 });
});

test('posting: the media X takes with one post, and the text the new post is found by', async () => {
  const { mediaForPage, fingerprint } = await import('../lib/publish.mjs');
  const { writeFile } = await import('node:fs/promises');
  const root = await mkdtemp(join(tmpdir(), 'x-feed-post-'));
  try {
    const file = async (name) => {
      await writeFile(join(root, name), 'fake media bytes');
      return name;
    };
    const images = await Promise.all(['1.jpg', '2.png', '3.webp', '4.jpg', '5.jpg'].map(file));
    const four = await mediaForPage(images.slice(0, 4), root);
    assert.deepEqual(four.map((f) => f.type), ['image/jpeg', 'image/png', 'image/webp', 'image/jpeg']);
    assert.equal(Buffer.from(four[0].data, 'base64').toString(), 'fake media bytes');
    await assert.rejects(mediaForPage(images, root), /at most 4 images/);
    const video = await file('v.mp4');
    await assert.rejects(mediaForPage([video, images[0]], root), /one video/);
    await assert.rejects(mediaForPage([await file('notes.txt')], root), /not a file X takes/);
    assert.equal((await mediaForPage([video], root))[0].type, 'video/mp4');
    // Only the draft's own files: a path out of its folder is refused before anything is read.
    await assert.rejects(mediaForPage(['../elsewhere.jpg'], join(root, 'draft')), /not a file of this draft/);
    await assert.rejects(mediaForPage(['/etc/hosts.png'], root), /not a file of this draft/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
  assert.equal(fingerprint('🚀 Ship 41 moved\n\nhttps://t.co/x #SpaceX'), '🚀 Ship 41 moved');
});

test('two automations saving the same account at once both keep what they added', async () => {
  const root = await mkdtemp(join(tmpdir(), 'x-feed-race-'));
  try {
    const store = createStore(root);
    // Both read the record before either saves, as two automations running side by side do.
    const one = await store.account('x', 'someone');
    const two = await store.account('x', 'someone');
    one.known['1'] = { day: '2026-09-24', status: 'refined' };
    two.known['2'] = { day: '2026-09-24', status: 'refined' };
    await Promise.all([store.saveAccount(one), store.saveAccount(two)]);
    const saved = await store.account('x', 'someone');
    assert.deepEqual(Object.keys(saved.known).sort(), ['1', '2']);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('every action goes into the log of its day and comes back newest first', async () => {
  const root = await mkdtemp(join(tmpdir(), 'x-feed-log-'));
  try {
    const store = createStore(root);
    await store.appendAction({ at: '2026-09-23T10:00:00', action: 'open', url: 'https://x.com/a' });
    await store.appendAction({ at: '2026-09-24T10:00:00', action: 'like', url: 'https://x.com/a/status/1', ok: true });
    await store.appendAction({ at: '2026-09-24T10:01:00', action: 'reply', url: 'https://x.com/a/status/1', ok: false, detail: 'no reply box' });
    assert.deepEqual((await readdir(join(root, 'actions'))).sort(), ['2026-09-23.jsonl', '2026-09-24.jsonl']);
    const all = await store.actions();
    assert.deepEqual(all.map((e) => e.action), ['reply', 'like', 'open']);
    assert.equal(all[0].detail, 'no reply box');
    assert.deepEqual((await store.actions(2)).map((e) => e.action), ['reply', 'like']);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('what the page shows becomes a candidate; every string the plugin uses is in all languages', async () => {
  const { toCandidate, sameHandle, requiredPieces, pickCandidates } = await import('../lib/engage.mjs');
  const post = toCandidate({ id: '9', url: 'https://x.com/a/status/9', author: 'A', handle: '@A', time: null, text: 'hi', labels: { likes: '1,234 Likes. Like' } });
  assert.equal(post.likes, 1234);
  assert.equal(post.repost, false);
  const repost = toCandidate({ id: '8', handle: '@b', socialContext: 'A reposted', labels: {} });
  assert.equal(repost.repost, true);
  assert.deepEqual(pickCandidates([post, repost]).map((p) => p.id), ['9'], 'reposts are left out');
  assert.ok(sameHandle('@Someone', 'someone'));
  assert.deepEqual(requiredPieces(' https://a.b \n\n#tag '), ['https://a.b', '#tag']);

  const { TABLE } = await import('../lib/i18n.mjs');
  const source = await readFile(new URL('../main.mjs', import.meta.url), 'utf8');
  const used = [...source.matchAll(/\bt\('([a-z_.]+)'/g)].map((m) => m[1]);
  for (const key of used) {
    assert.ok(TABLE[key], `missing string ${key}`);
    assert.equal(TABLE[key].length, 4, `${key} in four languages`);
  }
});

test('what a stranger\'s post could make an agent write is not posted, and media come from X only', async () => {
  const { agentReplyProblems } = await import('../lib/engage.mjs');
  const { mediaHostAllowed } = await import('../lib/media.mjs');
  const post = { handle: '@someone' };
  assert.deepEqual(agentReplyProblems('Great point, @someone!', { post }), []);
  assert.deepEqual(agentReplyProblems('More at https://agentty.run', { required: ['https://agentty.run'], post }), []);
  assert.ok(agentReplyProblems('Claim it at https://evil.example/x', { post }).some((p) => p.startsWith('link')));
  assert.ok(agentReplyProblems('see evil.io now', { post }).some((p) => p.startsWith('link')));
  assert.ok(agentReplyProblems('cc @attacker', { post }).some((p) => p.startsWith('mention')));
  assert.ok(agentReplyProblems('a\nb\nc\nd\ne\nf\ng', { post }).includes('too many lines'));

  assert.ok(mediaHostAllowed('https://pbs.twimg.com/media/a.jpg?name=orig'));
  assert.ok(mediaHostAllowed('https://video.twimg.com/ext_tw_video/1/pu/vid/720x1280/a.mp4'));
  for (const bad of ['http://pbs.twimg.com/a.jpg', 'https://127.0.0.1/a.jpg', 'https://evil.example/twimg.com/a.jpg', 'https://twimg.com.evil.example/a.jpg', 'file:///etc/hosts', 'not a url']) {
    assert.ok(!mediaHostAllowed(bad), bad);
  }
});
