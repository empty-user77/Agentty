// X Feed Reader: reads an account's posts through the in-app browser the user is signed in to.
//
// Agentty keeps the sign-in (cookies never reach this plugin) and runs the page; this plugin only
// opens the profile, scrolls it and reads the posts off the page.
import { writeFile, mkdir, readFile, access } from 'node:fs/promises';
import { join } from 'node:path';
import { createPlugin, ui } from '../../agentty-plugin.mjs';

const plugin = createPlugin();

const state = {
  account: '',
  // Small by default: a person reads a handful of posts, not a hundred in one go.
  limit: 10,
  mode: 'auto',
  status: 'Idle.',
  busy: false,
  site: null,
  sites: [],
  posts: [],
  file: null,
};

// Runs inside the page (in Agentty's own script world: the page cannot see or change it).
// Reads the posts on screen; `args.seen` lists the ids already collected.
function readPosts(args) {
  const text = (el) => (el ? el.innerText.trim() : '');
  const count = (label) => {
    const match = /([\d.,]+\s*[KMB]?)/i.exec(label || '');
    return match ? match[1].replace(/\s+/g, '') : null;
  };
  const posts = [];
  for (const article of document.querySelectorAll('article[data-testid="tweet"], article[data-post]')) {
    const time = article.querySelector('time');
    const link = time ? time.closest('a') : article.querySelector('a[href*="/status/"]');
    const url = link ? new URL(link.getAttribute('href'), location.href).href : null;
    const id = article.getAttribute('data-post') || (url && (url.match(/status\/(\d+)/) || [])[1]) || null;
    if (!id || args.seen.includes(id)) continue;
    const names = text(article.querySelector('[data-testid="User-Name"], .author')).split('\n');
    const images = [...article.querySelectorAll('[data-testid="tweetPhoto"] img, img.media')].map((img) => img.src);
    const videos = [...article.querySelectorAll('video')].map((video) => ({ src: video.currentSrc || video.src || null, poster: video.poster || null }));
    const metric = (name) => count((article.querySelector(`[data-testid="${name}"]`) || {}).getAttribute?.('aria-label'));
    posts.push({
      id,
      url,
      author: names[0] || null,
      handle: names.find((n) => n.startsWith('@')) || null,
      time: time ? time.getAttribute('datetime') : null,
      text: text(article.querySelector('[data-testid="tweetText"], .text')),
      // "Pinned", "X reposted"… in the reader's language, as the page shows it.
      socialContext: text(article.querySelector('[data-testid="socialContext"]')) || null,
      images,
      videos,
      replies: metric('reply'),
      reposts: metric('retweet'),
      likes: metric('like'),
    });
  }
  return {
    posts,
    url: location.href,
    signInWall: !!document.querySelector('[data-testid="loginButton"], a[href="/login"], form[action*="login"]'),
    end: window.innerHeight + window.scrollY >= document.body.scrollHeight - 4,
  };
}

// Scrolls like a hand on a trackpad: a few uneven steps, not one jump of a whole screen.
async function scrollDown() {
  const steps = 3 + Math.floor(Math.random() * 4);
  const distance = window.innerHeight * (0.45 + Math.random() * 0.4);
  for (let i = 0; i < steps; i += 1) {
    window.scrollBy({ top: distance / steps, behavior: 'smooth' });
    await new Promise((resolve) => setTimeout(resolve, 120 + Math.random() * 260));
  }
  return window.scrollY;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
/** A reader's pause: mostly a few seconds, sometimes longer, never the same twice. */
const readingPause = () => sleep(2500 + Math.random() * 3500 + (Math.random() < 0.2 ? 4000 + Math.random() * 5000 : 0));

/** Local date, YYYY-MM-DD: the folder a day's collection goes in. */
function today() {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

/** Folder-safe name of what was read: `elonmusk` for x.com/elonmusk, the host otherwise. */
function sourceParts(url) {
  const u = new URL(url);
  const site = /(^|\.)(x|twitter)\.com$/.test(u.hostname) ? 'x' : u.hostname.replace(/[^a-z0-9.-]/gi, '_');
  const account = (u.pathname.split('/').filter(Boolean)[0] || 'home').replace(/[^a-z0-9_.-]/gi, '_');
  return { site, account };
}

/** The largest version X serves of an image (`name=orig`); other addresses as they are. */
function fullSize(src) {
  try {
    const u = new URL(src);
    if (u.hostname === 'pbs.twimg.com' && u.pathname.startsWith('/media/')) u.searchParams.set('name', 'orig');
    return u.href;
  } catch {
    return src;
  }
}

function extension(url, type) {
  const format = (() => { try { return new URL(url).searchParams.get('format'); } catch { return null; } })();
  const fromType = { 'image/jpeg': 'jpg', 'image/png': 'png', 'image/webp': 'webp', 'image/gif': 'gif' }[type?.split(';')[0]];
  const fromPath = /\.(jpe?g|png|webp|gif)$/i.exec(new URL(url).pathname)?.[1];
  return (format || fromType || fromPath || 'jpg').replace('jpeg', 'jpg');
}

const exists = (path) => access(path).then(() => true, () => false);

/** Downloads a post's images into `dir/images`, one at a time and unhurried; returns their paths. */
async function downloadImages(post, dir) {
  const saved = [];
  for (const [index, src] of post.images.entries()) {
    if (!/^https?:/.test(src)) continue;
    const url = fullSize(src);
    const base = `${post.id}_${index + 1}`;
    try {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const file = `images/${base}.${extension(url, response.headers.get('content-type'))}`;
      if (!(await exists(join(dir, file)))) {
        await writeFile(join(dir, file), Buffer.from(await response.arrayBuffer()));
      }
      saved.push(file);
    } catch (err) {
      plugin.log(`image ${url}: ${err.message}`);
    }
    await sleep(250 + Math.random() * 500);
  }
  return saved;
}

/** Keeps a day's collection in one place: posts.json (merged by id) and images/. */
async function saveCollection(url, posts) {
  const root = plugin.info?.plugin?.dataDir;
  if (!root) return null;
  const { site, account } = sourceParts(url);
  const dir = join(root, site, today(), account);
  await mkdir(join(dir, 'images'), { recursive: true });
  for (const [index, post] of posts.entries()) {
    state.status = `Saving images… (${index + 1}/${posts.length})`;
    await render();
    post.localImages = await downloadImages(post, dir);
    // A video plays from a stream the page assembles (blob:); only its poster is kept.
    post.videos = post.videos.map((video) => ({ ...video, src: video.src?.startsWith('blob:') ? null : video.src }));
  }
  const file = join(dir, 'posts.json');
  const earlier = await readFile(file, 'utf8').then((text) => JSON.parse(text).posts ?? [], () => []);
  const merged = [...posts, ...earlier.filter((old) => !posts.some((post) => post.id === old.id))];
  await writeFile(file, JSON.stringify({ source: url, site, account, day: today(), readAt: new Date().toISOString(), posts: merged }, null, 2));
  return file;
}

function profileUrl(account) {
  const value = account.trim();
  if (/^https?:\/\//.test(value)) return value;
  return `https://x.com/${value.replace(/^@/, '')}`;
}

async function refreshSites() {
  try {
    state.sites = await plugin.browser.sites();
    state.site = state.sites.find((site) => site.host === 'x.com') ?? null;
  } catch (err) {
    state.status = `Could not read the sign-in: ${err.message}`;
  }
}

function render() {
  const badge = (site) =>
    ui.badge(
      site.signedIn
        ? `${site.host}: signed in${site.expiresAt ? ` until ${new Date(site.expiresAt).toLocaleDateString()}` : ' (session)'}`
        : `${site.host}: not signed in`,
      site.signedIn ? 'success' : 'warning',
    );
  return plugin.setPanel(
    ui.column([
      ui.text('X Feed Reader', 'title'),
      ui.row(state.sites.map(badge), { gap: 'small' }),
      ui.button('signin', 'Sign in to x.com', { icon: 'key-round' }),
      ui.input('account', { placeholder: '@account, or a profile / local page URL', value: state.account }),
      ui.row([
        ui.choice('mode', [
          { value: 'auto', label: 'Auto' },
          { value: 'background', label: 'Background' },
          { value: 'visible', label: 'Visible' },
        ], state.mode),
        ui.choice('limit', [5, 10, 20].map((n) => ({ value: String(n), label: `${n} posts` })), String(state.limit)),
      ]),
      ui.button('read', state.busy ? 'Reading…' : 'Read posts', { icon: 'play', variant: 'primary', disabled: state.busy }),
      state.busy ? ui.spinner(state.status) : ui.text(state.status, 'muted'),
      state.file ? ui.text(`Saved to ${state.file}`, 'small') : ui.text('', 'small'),
      ui.list(
        'posts',
        state.posts.map((post) => ({
          id: post.id,
          title: `${post.author ?? ''} ${post.handle ?? ''}`.trim() || post.id,
          subtitle: (post.text || '(no text)').slice(0, 140),
          detail: [post.time && new Date(post.time).toLocaleString(), post.images.length && `${post.images.length} image(s)`, post.videos.length && `${post.videos.length} video(s)`, post.likes && `♥ ${post.likes}`]
            .filter(Boolean)
            .join(' · '),
        })),
        { empty: 'No posts read yet.' },
      ),
    ]),
  );
}

async function read() {
  if (state.busy) return;
  if (!state.account.trim()) {
    state.status = 'Enter an account first.';
    return render();
  }
  state.busy = true;
  state.posts = [];
  state.file = null;
  state.status = 'Opening the profile…';
  await render();
  let tabId = null;
  try {
    const url = profileUrl(state.account);
    ({ tabId } = await plugin.browser.open(url, { mode: state.mode }));
    await plugin.browser.wait(tabId, { timeoutMs: 30000 });
    const seen = [];
    let idle = 0;
    while (state.posts.length < state.limit && idle < 6) {
      // Posts arrive after the page has loaded, and more after each scroll; a person takes a
      // moment to read them before moving on.
      await readingPause();
      const page = await plugin.browser.eval(tabId, readPosts, { seen }, { timeoutMs: 20000 });
      if (page.signInWall && state.posts.length === 0) {
        const { site } = await plugin.browser.info(tabId);
        state.status = `${site} asks to sign in. Sign in in the browser tab.`;
        await render();
        const result = await plugin.browser.signIn(site, { message: 'Sign in to read posts' });
        await refreshSites();
        if (!result.signedIn) {
          state.status = `Not signed in to ${site} (${result.reason ?? 'unknown'}).`;
          return;
        }
        state.status = 'Signed in: reading…';
        await plugin.browser.navigate(tabId, url);
        await plugin.browser.wait(tabId, { timeoutMs: 30000 });
        continue;
      }
      for (const post of page.posts) {
        seen.push(post.id);
        state.posts.push(post);
      }
      idle = page.posts.length === 0 ? idle + 1 : 0;
      state.status = `Read ${state.posts.length} post(s)…`;
      await render();
      if (state.posts.length >= state.limit) break;
      await plugin.browser.eval(tabId, scrollDown);
    }
    state.posts = state.posts.slice(0, state.limit);
    // Out of sight the page has done its job; in a tab the user is watching, it stays for them.
    if (state.mode !== 'visible') {
      await plugin.browser.close(tabId).catch(() => {});
      tabId = null;
    }
    state.file = await saveCollection(url, state.posts);
    const images = state.posts.reduce((n, post) => n + (post.localImages?.length ?? 0), 0);
    state.status = `Done: ${state.posts.length} post(s), ${images} image(s).`;
  } catch (err) {
    state.status = `Failed: ${err.message}`;
    plugin.log(err?.stack ?? String(err));
  } finally {
    state.busy = false;
    if (tabId !== null && state.mode !== 'visible') await plugin.browser.close(tabId).catch(() => {});
    await render();
  }
}

plugin
  .onPanelOpen(async () => {
    await refreshSites();
    await render();
  })
  .onEvent('account', (event) => {
    state.account = event.value ?? '';
    if (event.event === 'submit') return read();
  })
  .onEvent('mode', (event) => {
    state.mode = event.value;
    return render();
  })
  .onEvent('limit', (event) => {
    state.limit = Math.min(Number(event.value) || 10, 20);
    return render();
  })
  .onEvent('read', () => read())
  .onEvent('signin', async () => {
    state.status = 'Waiting for you to sign in to x.com in the browser tab…';
    await render();
    const result = await plugin.browser.signIn('x.com', { message: 'Sign in to x.com' });
    state.status = result.signedIn ? 'Signed in.' : `Not signed in (${result.reason ?? 'unknown'}).`;
    await refreshSites();
    await render();
  })
  .onBrowserHidden(() => {})
  .start();
