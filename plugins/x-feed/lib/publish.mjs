// Posting a draft: its text and media, from the automation's page, as a person would — the
// compose window, the text typed in, the files picked, the Post button — then the new post's
// address, read back from the user's own timeline.
//
// The page functions run inside X's page through `plugin.browser.eval`; each is self-contained.

import { readFile } from 'node:fs/promises';
import { extname, basename, resolve, sep } from 'node:path';

/** What X takes with one post: up to 4 images, or 1 video (or GIF). */
export const MAX_IMAGES = 4;
/** Media travel into the page as text (base64) in one message, which the host caps at 16 MB. */
export const MAX_MEDIA_BYTES = 11 * 1024 * 1024;

const TYPES = { '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.png': 'image/png', '.webp': 'image/webp', '.gif': 'image/gif', '.mp4': 'video/mp4', '.mov': 'video/quicktime' };

/**
 * The files of a draft, read for the page: [{ name, type, data (base64) }]. `paths` are relative
 * to the draft's folder `dir` and must stay inside it: nothing else on disk is ever uploaded.
 */
export async function mediaForPage(paths, dir) {
  const files = [];
  let bytes = 0;
  const root = resolve(dir);
  for (const relative of paths) {
    const path = resolve(root, String(relative));
    if (!path.startsWith(`${root}${sep}`)) throw new Error(`not a file of this draft: ${relative}`);
    const type = TYPES[extname(path).toLowerCase()];
    if (!type) throw new Error(`not a file X takes: ${basename(path)}`);
    const data = await readFile(path);
    bytes += data.length;
    if (bytes > MAX_MEDIA_BYTES) throw new Error(`the media are over ${MAX_MEDIA_BYTES / 1024 / 1024} MB`);
    files.push({ name: basename(path), type, data: data.toString('base64') });
  }
  const videos = files.filter((f) => f.type.startsWith('video/') || f.type === 'image/gif').length;
  if (videos > 1 || (videos && files.length > 1)) throw new Error('a post takes one video or GIF, alone');
  if (files.length > MAX_IMAGES) throw new Error(`a post takes at most ${MAX_IMAGES} images`);
  return files;
}

/**
 * Whether the compose window holds `expected`: the same words in the same order, spacing aside
 * (the editor shows line breaks its own way). Nothing is posted otherwise.
 */
export function sameText(shown, expected) {
  const flat = (text) => String(text ?? '').replace(/\s+/g, '');
  return flat(shown) === flat(expected);
}

/** The start of a post's text as a timeline shows it, to find the new post by. */
export function fingerprint(text) {
  return String(text ?? '').replace(/https?:\/\/\S+/g, '').replace(/#\S+/g, '').replace(/\s+/g, ' ').trim().slice(0, 40);
}

// ---------------------------------------------------------------------------------------------
// In the page
// ---------------------------------------------------------------------------------------------

/**
 * Opens the compose window, types `args.text` and attaches `args.files`. { ok, error, step }.
 * Uploading goes on after this returns: `composeState` says when it is done.
 */
export async function composeInPage(args) {
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const dialog = () => document.querySelector('[role="dialog"] [data-testid="tweetTextarea_0"]')?.closest('[role="dialog"]') ?? null;
  if (dialog()) return { ok: false, step: 'open', error: 'a compose window is already open' };
  const open = document.querySelector('[data-testid="SideNav_NewTweet_Button"]');
  if (!open) return { ok: false, step: 'open', error: 'no Post button on the page' };
  open.click();
  let box = null;
  for (let waited = 0; waited < 10000 && !box; waited += 250) {
    box = dialog()?.querySelector('[data-testid="tweetTextarea_0"] [contenteditable="true"], [data-testid="tweetTextarea_0"][contenteditable="true"]');
    if (!box) await wait(250);
  }
  if (!box) return { ok: false, step: 'open', error: 'the compose window did not open' };
  box.focus();
  await wait(300 + Math.random() * 400);
  // As a paste: X's editor takes line breaks from a paste, not from typed-in text.
  const clip = new DataTransfer();
  clip.setData('text/plain', args.text);
  box.dispatchEvent(new ClipboardEvent('paste', { clipboardData: clip, bubbles: true, cancelable: true }));
  await wait(500 + Math.random() * 500);
  if (args.files?.length) {
    const input = dialog().querySelector('input[data-testid="fileInput"]') ?? document.querySelector('input[data-testid="fileInput"]');
    if (!input) return { ok: false, step: 'media', error: 'no file input' };
    const transfer = new DataTransfer();
    for (const file of args.files) {
      const bytes = Uint8Array.from(atob(file.data), (c) => c.charCodeAt(0));
      transfer.items.add(new File([bytes], file.name, { type: file.type }));
    }
    input.files = transfer.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }
  return { ok: true };
}

/** The compose window now: open, what it holds, whether it is still uploading, whether Post is on. */
export function composeState() {
  const dialog = document.querySelector('[role="dialog"] [data-testid="tweetTextarea_0"]')?.closest('[role="dialog"]') ?? null;
  if (!dialog) return { open: false };
  const button = dialog.querySelector('[data-testid="tweetButton"]');
  const toast = document.querySelector('[data-testid="toast"]');
  return {
    open: true,
    text: (dialog.querySelector('[data-testid="tweetTextarea_0"]')?.innerText ?? '').trim(),
    media: dialog.querySelectorAll('[data-testid="attachments"] img, [data-testid="attachments"] video').length,
    // The attachments' own progress (the round character counter is a progress bar too).
    uploading: !!dialog.querySelector('[data-testid="attachments"] [role="progressbar"]'),
    canPost: !!button && button.getAttribute('aria-disabled') !== 'true' && !button.disabled,
    message: toast ? toast.innerText.trim().slice(0, 200) : null,
  };
}

/** Presses Post in the compose window: { pressed }. */
export function pressPost() {
  const dialog = document.querySelector('[role="dialog"] [data-testid="tweetTextarea_0"]')?.closest('[role="dialog"]') ?? null;
  const button = dialog?.querySelector('[data-testid="tweetButton"]');
  if (!button || button.getAttribute('aria-disabled') === 'true') return { pressed: false };
  button.click();
  return { pressed: true };
}

/** Closes the compose window without posting (X may ask to save a draft: not saved). */
export async function discardCompose() {
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const close = document.querySelector('[role="dialog"] [data-testid="app-bar-close"]');
  if (!close) return { closed: false };
  close.click();
  await wait(600);
  document.querySelector('[data-testid="confirmationSheetCancel"]')?.click();
  await wait(400);
  return { closed: !document.querySelector('[role="dialog"] [data-testid="tweetTextarea_0"]') };
}
