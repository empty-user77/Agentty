// What is X-specific: reading a profile's timeline off the page, scrolling it like a person, and
// the public embed data of a post (its full text, quoted post and video files), which the page
// itself only shows in pieces.
//
// The page functions run inside the page through `plugin.browser.eval`, one at a time, so each
// must be self-contained: nothing from this module is in scope there.

/** Reads what the profile page shows now. `args.seen` lists ids already taken this run. */
export function readTimeline(args) {
  const text = (el) => (el ? el.innerText.trim() : '');
  const label = (el) => (el ? el.getAttribute('aria-label') || '' : '');
  const posts = [];
  const articles = [...document.querySelectorAll('article[data-testid="tweet"], article[data-post]')];
  for (const [index, article] of articles.entries()) {
    const time = article.querySelector('time');
    const link = time ? time.closest('a') : article.querySelector('a[href*="/status/"]');
    const url = link ? new URL(link.getAttribute('href'), location.href).href : null;
    const id = article.getAttribute('data-post') || (url && (url.match(/status\/(\d+)/) || [])[1]) || null;
    if (!id || args.seen.includes(id)) continue;
    const names = text(article.querySelector('[data-testid="User-Name"], .author')).split('\n');
    const metric = (name) => label(article.querySelector(`[data-testid="${name}"]`));
    const views = article.querySelector('a[href$="/analytics"]');
    posts.push({
      id,
      url,
      index,
      author: names[0] || null,
      handle: names.find((n) => n.startsWith('@')) || null,
      time: time ? time.getAttribute('datetime') : null,
      text: text(article.querySelector('[data-testid="tweetText"], .text')),
      socialContext: text(article.querySelector('[data-testid="socialContext"]')) || null,
      images: [...article.querySelectorAll('[data-testid="tweetPhoto"] img, img.media')].map((img) => img.src),
      videos: [...article.querySelectorAll('video')].map((video) => ({ src: video.currentSrc || video.src || null, poster: video.poster || null })),
      hasQuote: !!article.querySelector('[data-testid="quoteTweet"], div[role="link"] time'),
      labels: { replies: metric('reply'), reposts: metric('retweet'), likes: metric('like'), views: label(views) },
    });
  }
  const empty = document.querySelector('[data-testid="emptyState"]');
  const error = document.querySelector('[data-testid="error-detail"], [data-testid="primaryColumn"] [role="alert"]');
  return {
    posts,
    url: location.href,
    signInWall: !!document.querySelector('[data-testid="loginButton"], a[href="/login"], form[action*="login"]'),
    empty: empty ? empty.innerText.trim().slice(0, 200) : null,
    error: error ? error.innerText.trim().slice(0, 200) : null,
    atEnd: window.innerHeight + window.scrollY >= document.body.scrollHeight - 4,
  };
}

/** Scrolls like a hand on a trackpad: a few uneven steps, not one jump of a whole screen. */
export async function scrollLikeAHand() {
  const steps = 3 + Math.floor(Math.random() * 4);
  const distance = window.innerHeight * (0.45 + Math.random() * 0.4);
  for (let i = 0; i < steps; i += 1) {
    window.scrollBy({ top: distance / steps, behavior: 'smooth' });
    await new Promise((resolve) => setTimeout(resolve, 120 + Math.random() * 260));
  }
  return window.scrollY;
}

/** The profile page of `account` (`@name`, `name`, or a full URL of a page on x.com). */
export function profileUrl(account) {
  const value = String(account).trim();
  if (/^https?:\/\//.test(value)) return value;
  return `https://x.com/${value.replace(/^@/, '')}`;
}

/** The account a profile URL is for: `elonmusk` for https://x.com/elonmusk. */
export function accountOf(url) {
  try {
    const u = new URL(url);
    return u.pathname.split('/').filter(Boolean)[0] || u.hostname;
  } catch {
    return String(url).replace(/^@/, '');
  }
}

/** The pinned post comes first but is older than the one after it: that is how it is told apart. */
export function markPinned(posts) {
  if (posts.length < 2) return posts;
  const [first, second] = posts;
  if (first.index === 0 && first.time && second.time && first.time < second.time) first.pinned = true;
  return posts;
}

/** The token the embed endpoint expects for a post id (what X's own embed script computes). */
export function embedToken(id) {
  return ((Number(id) / 1e15) * Math.PI).toString(36).replace(/(0+|\.)/g, '');
}

/** The public embed data of a post, or null when X does not give it (deleted, protected, limits). */
export async function fetchEmbed(id, { fetchImpl = fetch, language = 'en' } = {}) {
  const url = `https://cdn.syndication.twimg.com/tweet-result?id=${encodeURIComponent(id)}&lang=${language}&token=${embedToken(id)}`;
  try {
    const response = await fetchImpl(url, { headers: { accept: 'application/json' } });
    if (!response.ok) return null;
    const data = await response.json();
    return data && data.id_str ? data : null;
  } catch {
    return null;
  }
}

/**
 * The mp4 to keep of a video: the best one no taller than `maxHeight` (a 4K file is rarely worth
 * its size), else the smallest there is.
 */
export function pickVideo(variants, maxHeight = 1920) {
  const mp4 = (variants ?? []).filter((v) => v.content_type === 'video/mp4' && v.url);
  if (!mp4.length) return null;
  const height = (v) => Number((/\/(\d+)x(\d+)\//.exec(v.url) || [])[2]) || 0;
  const fitting = mp4.filter((v) => height(v) <= maxHeight);
  const pool = fitting.length ? fitting : mp4;
  return pool.sort((a, b) => (fitting.length ? (b.bitrate ?? 0) - (a.bitrate ?? 0) : (a.bitrate ?? 0) - (b.bitrate ?? 0)))[0];
}

/** The largest version X serves of an image (`name=orig`); other addresses as they are. */
export function fullSizeImage(src) {
  try {
    const u = new URL(src);
    if (u.hostname === 'pbs.twimg.com' && u.pathname.startsWith('/media/')) u.searchParams.set('name', 'orig');
    return u.href;
  } catch {
    return src;
  }
}
