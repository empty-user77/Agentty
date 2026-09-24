// Refining: from what the page showed (and the post's embed data, when X gives it) to one clean
// record per post — its kind, its whole text with links expanded, its tags, numbers that are
// numbers, and the media to download.

const ENTITIES = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#39;': "'", '&apos;': "'" };

export function unescapeHtml(text) {
  return String(text ?? '').replace(/&(amp|lt|gt|quot|#39|apos);/g, (m) => ENTITIES[m]);
}

/**
 * A count as the page writes it, in any of the languages X speaks: `3,174`, `1.2K`, `4.5M`,
 * `1.2만`, `3천`, `12万`, or an aria-label like "3174 replies" / "답글 3,174개". `null` when there is
 * no number at all.
 */
export function parseCount(label) {
  const match = /(\d[\d,.\s]*)\s*([KMB]|천|만|억|万|億)?/i.exec(String(label ?? ''));
  if (!match) return null;
  const digits = match[1].replace(/[\s,]/g, '');
  const unit = (match[2] || '').toUpperCase();
  const scale = { K: 1e3, M: 1e6, B: 1e9, 천: 1e3, 만: 1e4, 억: 1e8, 万: 1e4, 億: 1e8 }[unit] ?? 1;
  // A dot without a unit is a thousands separator in some locales ("3.174").
  const value = unit ? Number(digits) : Number(digits.replace(/\./g, ''));
  return Number.isFinite(value) ? Math.round(value * scale) : null;
}

/** The part of the embed text the post shows: no leading @replies, no trailing media link. */
function displayText(embed) {
  // Only the start of `display_text_range` is used (it skips the @names a reply begins with): its
  // end is counted on text that may or may not be escaped, and cutting there can lose characters.
  // The trailing media link is removed by name below instead.
  const text = Array.from(unescapeHtml(embed.text ?? ''));
  const [start] = embed.display_text_range ?? [0];
  let shown = text.slice(start).join('');
  for (const link of embed.entities?.urls ?? []) {
    if (link.url && link.expanded_url) shown = shown.split(link.url).join(link.expanded_url);
  }
  for (const media of embed.entities?.media ?? []) {
    if (media.url) shown = shown.split(media.url).join('');
  }
  return shown.trim();
}

function embedMedia(embed, fromQuote) {
  return (embed?.mediaDetails ?? []).map((media) => ({
    kind: media.type === 'photo' ? 'image' : media.type === 'animated_gif' ? 'gif' : 'video',
    source: media.type === 'photo' ? media.media_url_https : null,
    poster: media.type === 'photo' ? null : media.media_url_https,
    variants: media.video_info?.variants ?? [],
    durationMs: media.video_info?.duration_millis ?? null,
    width: media.original_info?.width ?? null,
    height: media.original_info?.height ?? null,
    alt: media.ext_alt_text ?? null,
    fromQuote,
  }));
}

function quoteOf(embed) {
  const quoted = embed?.quoted_tweet;
  if (!quoted) return null;
  return {
    id: quoted.id_str,
    url: quoted.user?.screen_name ? `https://x.com/${quoted.user.screen_name}/status/${quoted.id_str}` : null,
    author: { name: quoted.user?.name ?? null, handle: quoted.user?.screen_name ? `@${quoted.user.screen_name}` : null },
    text: displayText(quoted),
    postedAt: quoted.created_at ? new Date(quoted.created_at).toISOString() : null,
  };
}

/** What kind of post this is on the account's timeline. */
export function kindOf(seen, embed, account) {
  const handle = (seen.handle || '').replace(/^@/, '').toLowerCase();
  if (handle && account && handle !== account.toLowerCase()) return 'repost';
  if (embed?.in_reply_to_screen_name) return 'reply';
  if (embed?.quoted_tweet || seen.hasQuote) return 'quote';
  return 'post';
}

/**
 * One clean record from what the page showed (`seen`) and the embed data (`embed`, may be null).
 * `context`: { site, account, day, runId, collectedAt }.
 */
export function refine(seen, embed, context) {
  const text = embed ? displayText(embed) : seen.text || '';
  const media = embed
    ? [...embedMedia(embed, false), ...embedMedia(embed.quoted_tweet, true)]
    : [
        ...seen.images.map((source) => ({ kind: 'image', source, poster: null, variants: [], fromQuote: false })),
        ...seen.videos.map((video) => ({ kind: 'video', source: null, poster: video.poster, variants: [], fromQuote: false })),
      ];
  const hashtags = embed?.entities?.hashtags?.map((tag) => tag.text) ?? [...text.matchAll(/#([\p{L}\p{N}_]+)/gu)].map((m) => m[1]);
  const mentions = embed?.entities?.user_mentions?.map((user) => `@${user.screen_name}`) ?? [...text.matchAll(/@(\w{1,15})/g)].map((m) => `@${m[1]}`);
  const links = embed?.entities?.urls?.map((link) => link.expanded_url).filter(Boolean) ?? [...text.matchAll(/https?:\/\/\S+/g)].map((m) => m[0]);
  return {
    id: seen.id,
    site: context.site,
    account: context.account,
    day: context.day,
    runId: context.runId,
    collectedAt: context.collectedAt,
    url: seen.url ?? `https://x.com/${context.account}/status/${seen.id}`,
    kind: kindOf(seen, embed, context.account),
    pinned: !!seen.pinned,
    socialContext: seen.socialContext ?? null,
    author: {
      name: embed?.user?.name ?? seen.author ?? null,
      handle: embed?.user?.screen_name ? `@${embed.user.screen_name}` : seen.handle ?? null,
    },
    postedAt: embed?.created_at ? new Date(embed.created_at).toISOString() : seen.time ?? null,
    lang: embed?.lang ?? null,
    text,
    replyTo: embed?.in_reply_to_screen_name ? `@${embed.in_reply_to_screen_name}` : null,
    quote: quoteOf(embed),
    hashtags: [...new Set(hashtags)],
    mentions: [...new Set(mentions)],
    links: [...new Set(links)],
    metrics: {
      replies: parseCount(seen.labels?.replies) ?? embed?.conversation_count ?? null,
      reposts: parseCount(seen.labels?.reposts),
      likes: parseCount(seen.labels?.likes) ?? embed?.favorite_count ?? null,
      views: parseCount(seen.labels?.views),
      at: context.collectedAt,
    },
    media,
    source: embed ? 'page+embed' : 'page',
    status: 'refined',
    statusAt: context.collectedAt,
  };
}

const yaml = (value) => JSON.stringify(value ?? null);

/** The post as a page a person reads, with its media next to it (paths relative to its folder). */
export function toMarkdown(post) {
  const lines = [
    '---',
    `id: ${yaml(post.id)}`,
    `url: ${yaml(post.url)}`,
    `author: ${yaml(`${post.author.name ?? ''} ${post.author.handle ?? ''}`.trim())}`,
    `posted: ${yaml(post.postedAt)}`,
    `kind: ${yaml(post.kind)}${post.pinned ? ' # pinned' : ''}`,
    `status: ${yaml(post.status)}`,
    `collected: ${yaml(post.collectedAt)}`,
    `metrics: { replies: ${post.metrics.replies ?? 'null'}, reposts: ${post.metrics.reposts ?? 'null'}, likes: ${post.metrics.likes ?? 'null'}, views: ${post.metrics.views ?? 'null'} }`,
    post.hashtags.length ? `hashtags: ${yaml(post.hashtags)}` : null,
    '---',
    '',
    post.replyTo ? `> Replying to ${post.replyTo}\n` : null,
    post.text || '_(no text)_',
    '',
  ].filter((line) => line !== null);
  if (post.quote) {
    lines.push(`> **${post.quote.author.name ?? ''} ${post.quote.author.handle ?? ''}**`, ...post.quote.text.split('\n').map((l) => `> ${l}`), `> ${post.quote.url ?? ''}`, '');
  }
  for (const media of post.media) {
    if (media.kind === 'image' && media.path) lines.push(`![image](${media.path})`);
    else if (media.path) lines.push(`[${media.kind}](${media.path})${media.posterPath ? ` · ![poster](${media.posterPath})` : ''}`);
    else if (media.posterPath) lines.push(`![${media.kind} poster](${media.posterPath}) _(${media.kind} not downloaded)_`);
  }
  if (post.links.length) lines.push('', ...post.links.map((link) => `- ${link}`));
  lines.push('', `[Original post](${post.url})`, '');
  return lines.join('\n');
}
