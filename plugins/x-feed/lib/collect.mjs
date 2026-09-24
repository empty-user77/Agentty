// One collection run of one account: read the profile like a person would, take only what has not
// been collected before, refine it, download its media, and write down what the run did.
//
// An account is collected many times. Between two runs it may have posted nothing, a few posts, or
// more than one run takes — so a run stops at the first posts it already has ("caught up"), at its
// limit (and then says the next run may find a gap to fill), or where the timeline ends.

import { join } from 'node:path';
import { accountOf, appReady, fetchEmbed as fetchEmbedDefault, goToPath, HOME, markPinned, profileUrl, readTimeline, scrollLikeAHand } from './x.mjs';
import { refine, toMarkdown } from './refine.mjs';
import { downloadMedia } from './media.mjs';
import { day as dayOf, newId } from './store.mjs';

/** Posts a run takes at most, whatever it is asked for: a person reads a few, not hundreds. */
export const MAX_PER_RUN = 20;
/** Known posts in a row (the pinned one aside) that mean everything newer has been read. */
const CAUGHT_UP_AFTER = 3;
/** Reads that brought nothing new before the timeline counts as ended. */
const IDLE_READS = 4;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
/** A reader's pause: mostly a few seconds, sometimes longer, never the same twice. */
export const readingPause = () => sleep(2500 + Math.random() * 3500 + (Math.random() < 0.2 ? 4000 + Math.random() * 5000 : 0));

/**
 * Reads new posts of `account` off its profile.
 *
 * `browser` is `plugin.browser`; `options`: { limit, mode, backfill, pause, fetchEmbed, fetchImpl,
 * progress, log, now, tabId, profile, instance, engine }. With `tabId` the run reads in that page (an
 * automation's own tab) and leaves it open; otherwise it opens one in `profile` and closes it. Returns the run record, which is also saved. A run that meets the sign-in
 * wall before reading anything stops with `stoppedBecause: "signin"` and the page left open
 * (`tabId`), for the caller to ask the user.
 */
export async function collect(browser, store, account, options = {}) {
  const site = 'x';
  const url = profileUrl(account);
  const name = accountOf(url);
  const limit = Math.max(1, Math.min(options.limit ?? 10, MAX_PER_RUN));
  const pause = options.pause ?? readingPause;
  const progress = options.progress ?? (() => {});
  const log = options.log ?? (() => {});
  const now = options.now ?? (() => new Date());
  const record = await store.account(site, name);
  const hadHistory = Object.keys(record.known).length > 0;
  const started = now();
  const run = {
    runId: newId('run'),
    account: name,
    day: dayOf(started),
    startedAt: started.toISOString(),
    endedAt: null,
    mode: options.mode ?? 'auto',
    limit,
    backfill: !!options.backfill,
    read: 0,
    new: 0,
    duplicates: 0,
    stoppedBecause: null,
    engine: options.engine ?? null,
    gap: false,
    error: null,
    postIds: [],
    failed: [],
  };

  const fresh = [];
  const seen = [];
  let tabId = options.tabId ?? null;
  const ownPage = tabId === null;
  let knownInARow = 0;
  let reachedKnown = false;
  let idle = 0;
  try {
    progress({ step: 'open', run });
    // In through the front door, then to the profile as a click would take a person there.
    if (ownPage) {
      ({ tabId } = await browser.open(HOME, { mode: options.mode, profile: options.profile, instance: options.instance }));
      await browser.wait(tabId, { timeoutMs: 30000 });
    }
    // A page stuck on X's splash (or somewhere else) starts again from home.
    const ready = await browser.eval(tabId, appReady).catch(() => false);
    const moved = ready && (await browser.eval(tabId, goToPath, { path: new URL(url).pathname }).catch(() => false));
    if (!moved) {
      await browser.navigate(tabId, HOME);
      await browser.wait(tabId, { timeoutMs: 30000 });
      await pause();
      await browser.eval(tabId, goToPath, { path: new URL(url).pathname });
    }
    // Enough reads for the limit, bounded however the page behaves.
    for (let reads = 0; reads < limit * 3 + 8 && !run.stoppedBecause; reads += 1) {
      await pause();
      const page = await browser.eval(tabId, readTimeline, { seen }, { timeoutMs: 20000 });
      if (page.signInWall && !seen.length) {
        run.stoppedBecause = 'signin';
        break;
      }
      if (page.error && !page.posts.length) {
        run.stoppedBecause = 'error';
        run.error = page.error;
        break;
      }
      if (page.empty && !page.posts.length && !seen.length) {
        run.stoppedBecause = 'empty';
        run.error = page.empty;
        break;
      }
      const posts = reads === 0 ? markPinned(page.posts) : page.posts;
      let newHere = 0;
      for (const post of posts) {
        seen.push(post.id);
        run.read += 1;
        if (record.known[post.id]) {
          run.duplicates += 1;
          if (!post.pinned) {
            knownInARow += 1;
            reachedKnown = true;
          }
          continue;
        }
        knownInARow = 0;
        newHere += 1;
        if (fresh.length < limit) fresh.push(post);
      }
      progress({ step: 'read', run, found: fresh.length });
      if (fresh.length >= limit) run.stoppedBecause = 'limit';
      else if (!run.backfill && knownInARow >= CAUGHT_UP_AFTER) run.stoppedBecause = 'caught-up';
      else if (newHere === 0 && (idle += 1) >= IDLE_READS) run.stoppedBecause = page.atEnd ? 'end' : 'no-more';
      else if (newHere > 0) idle = 0;
      if (!run.stoppedBecause) await browser.eval(tabId, scrollLikeAHand);
    }
    run.stoppedBecause ??= 'read-limit';
  } catch (err) {
    run.stoppedBecause = 'error';
    run.error = err.message;
    log(err.stack ?? String(err));
  }

  if (run.stoppedBecause === 'signin') {
    run.tabId = tabId;
    return run;
  }
  if (ownPage && tabId !== null && options.mode !== 'visible') await browser.close(tabId).catch(() => {});

  // Posts are taken in, one at a time, oldest of the run last so an interrupted run keeps the newest.
  for (const [index, seenPost] of fresh.entries()) {
    progress({ step: 'save', run, index, total: fresh.length });
    try {
      const embed = await (options.fetchEmbed ?? fetchEmbedDefault)(seenPost.id);
      const collectedAt = now().toISOString();
      let post = refine(seenPost, embed, { site, account: name, day: run.day, runId: run.runId, collectedAt });
      await store.saveRaw(site, run.day, name, post.id, { seen: seenPost, embed, collectedAt });
      post = await downloadMedia(post, store.postDir(site, run.day, name, post.id), { fetchImpl: options.fetchImpl, log });
      await store.savePost(post);
      await store.saveMarkdown(post, toMarkdown(post));
      record.known[post.id] = { day: run.day, status: post.status, postedAt: post.postedAt, kind: post.kind };
      await store.saveAccount(record);
      run.postIds.push(post.id);
      run.new += 1;
    } catch (err) {
      run.failed.push({ id: seenPost.id, error: err.message });
      log(`post ${seenPost.id}: ${err.stack ?? err.message}`);
    }
    if (index < fresh.length - 1) await pause();
  }

  // Stopped at the limit before meeting anything collected before: older new posts may be left
  // between this run and the last one. A run that reaches known posts (or the end) closes it.
  run.gap = run.stoppedBecause === 'limit' && hadHistory && !reachedKnown;
  if (run.gap) record.gap = { since: run.startedAt, runId: run.runId };
  else if (reachedKnown || run.stoppedBecause === 'end') record.gap = null;
  run.endedAt = now().toISOString();
  record.lastRun = { runId: run.runId, day: run.day, at: run.endedAt, new: run.new, duplicates: run.duplicates, stoppedBecause: run.stoppedBecause, error: run.error };
  record.runs = (record.runs ?? 0) + 1;
  await store.saveAccount(record);
  await store.addRun(site, run.day, name, run);
  return run;
}

/** Where a post's files are, for the UI's "open folder". */
export function postFolder(store, post) {
  return join(store.postDir(post.site, post.day, post.account, post.id));
}
