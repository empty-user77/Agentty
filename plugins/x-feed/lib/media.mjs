// Downloading a post's media next to it: images/ and videos/, one file at a time and unhurried.
// A file already there is not fetched again, so a run that stopped halfway picks up where it was.

import { createWriteStream } from 'node:fs';
import { access, mkdir, rename, rm, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { fullSizeImage, pickVideo } from './x.mjs';

/** The largest video kept; a longer one keeps its poster and says why. */
export const MAX_VIDEO_BYTES = 300 * 1024 * 1024;
const MAX_IMAGE_BYTES = 30 * 1024 * 1024;

const exists = (path) => access(path).then(() => true, () => false);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export function extensionOf(url, type) {
  let format = null;
  let path = '';
  try {
    const parsed = new URL(url);
    format = parsed.searchParams.get('format');
    path = parsed.pathname;
  } catch {
    // Not a URL: only the type can say.
  }
  const fromType = { 'image/jpeg': 'jpg', 'image/png': 'png', 'image/webp': 'webp', 'image/gif': 'gif', 'video/mp4': 'mp4' }[String(type ?? '').split(';')[0]];
  const fromPath = /\.(jpe?g|png|webp|gif|mp4)$/i.exec(path)?.[1];
  return String(format || fromType || fromPath || 'bin').toLowerCase().replace('jpeg', 'jpg');
}

/** Fetches `url` into `folder/base.<ext>` (streamed, size-capped); returns the file name. */
export async function download(url, folder, base, { maxBytes, fetchImpl = fetch, tries = 3 } = {}) {
  await mkdir(folder, { recursive: true });
  let lastError;
  for (let attempt = 1; attempt <= tries; attempt += 1) {
    try {
      const response = await fetchImpl(url);
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const length = Number(response.headers.get('content-length')) || 0;
      if (maxBytes && length > maxBytes) throw Object.assign(new Error(`too large (${Math.round(length / 1048576)} MB)`), { final: true });
      const name = `${base}.${extensionOf(url, response.headers.get('content-type'))}`;
      const target = join(folder, name);
      if (await exists(target)) {
        await response.body?.cancel?.();
        return name;
      }
      const partial = `${target}.part`;
      let received = 0;
      const counted = Readable.fromWeb(response.body).on('data', (chunk) => {
        received += chunk.length;
        if (maxBytes && received > maxBytes) counted.destroy(Object.assign(new Error('too large'), { final: true }));
      });
      await pipeline(counted, createWriteStream(partial));
      await rename(partial, target);
      return name;
    } catch (err) {
      lastError = err;
      await rm(join(folder, `${base}.part`), { force: true }).catch(() => {});
      if (err.final) break;
      await sleep(800 * attempt);
    }
  }
  throw lastError;
}

/**
 * Downloads every medium of `post` into its folder `dir` and records where each went (`path`,
 * `posterPath`) or why it could not (`error`). Returns the post with its media filled in.
 */
export async function downloadMedia(post, dir, { fetchImpl = fetch, pause = () => sleep(250 + Math.random() * 500), log = () => {} } = {}) {
  const media = [];
  let image = 0;
  let video = 0;
  for (const item of post.media) {
    const done = { ...item };
    try {
      if (item.kind === 'image' && item.source) {
        image += 1;
        const name = await download(fullSizeImage(item.source), join(dir, 'images'), String(image), { maxBytes: MAX_IMAGE_BYTES, fetchImpl });
        done.path = `images/${name}`;
      } else if (item.kind !== 'image') {
        video += 1;
        if (item.poster && /^https?:/.test(item.poster)) {
          const name = await download(fullSizeImage(item.poster), join(dir, 'images'), `video${video}-poster`, { maxBytes: MAX_IMAGE_BYTES, fetchImpl });
          done.posterPath = `images/${name}`;
        }
        const chosen = pickVideo(item.variants);
        if (chosen) {
          const name = await download(chosen.url, join(dir, 'videos'), String(video), { maxBytes: MAX_VIDEO_BYTES, fetchImpl });
          done.path = `videos/${name}`;
          done.bitrate = chosen.bitrate ?? null;
          done.bytes = (await stat(join(dir, 'videos', name))).size;
        } else {
          done.error = 'no downloadable file (only the poster was kept)';
        }
      }
    } catch (err) {
      done.error = err.message;
      log(`media of ${post.id}: ${err.message}`);
    }
    // Nothing X needs to see twice: the variants list is long and only the chosen file matters.
    delete done.variants;
    media.push(done);
    await pause();
  }
  return { ...post, media };
}
