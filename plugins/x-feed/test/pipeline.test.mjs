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
