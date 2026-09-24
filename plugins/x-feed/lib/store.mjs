// Where everything the plugin collects and makes is kept, and how it is read back.
//
//   <data>/x/accounts/<account>.json            what is known about an account across days
//   <data>/x/<day>/<account>/runs.json           every collection run of that day
//   <data>/x/<day>/<account>/posts/<id>/         raw.json, post.json, post.md, images/, videos/
//   <data>/drafts/<draft id>/                    draft.json, draft.md, media/
//   <data>/styles.json                           the styles drafts are made in
//
// Files are written whole and moved into place, so a crash leaves the old file or the new one,
// never half of either.

import { mkdir, readFile, writeFile, rename, readdir, rm, copyFile } from 'node:fs/promises';
import { join, dirname } from 'node:path';
import { randomBytes } from 'node:crypto';

/** Stages a post goes through, in order. */
export const POST_STATUS = ['refined', 'selected', 'skipped', 'drafted', 'uploaded'];
/** Stages a draft goes through: made, checked by the user, sent. */
export const DRAFT_STATUS = ['draft', 'ready', 'uploaded', 'discarded'];

/** Local date, YYYY-MM-DD: the folder a day's collection goes in. */
export function day(date = new Date()) {
  const pad = (n) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** A name that is safe as one folder: letters, digits, `_`, `.` and `-`. */
export function folderName(text) {
  const name = String(text ?? '').replace(/^@/, '').replace(/[^A-Za-z0-9_.-]/g, '_').replace(/^\.+/, '_');
  return name.slice(0, 80) || '_';
}

export function newId(prefix) {
  return `${prefix}-${Date.now().toString(36)}-${randomBytes(3).toString('hex')}`;
}

export async function readJson(file, fallback) {
  try {
    return JSON.parse(await readFile(file, 'utf8'));
  } catch (err) {
    if (err.code === 'ENOENT') return fallback;
    // Unreadable: kept aside rather than written over, so nothing collected is lost.
    if (err instanceof SyntaxError) await rename(file, `${file}.unreadable-${Date.now()}`).catch(() => {});
    return fallback;
  }
}

export async function writeJson(file, value) {
  await writeText(file, `${JSON.stringify(value, null, 2)}\n`);
}

export async function writeText(file, text) {
  await mkdir(dirname(file), { recursive: true });
  const tmp = `${file}.tmp-${process.pid}-${randomBytes(3).toString('hex')}`;
  await writeFile(tmp, text);
  await rename(tmp, file);
}

export function createStore(root) {
  const siteDir = (site) => join(root, folderName(site));
  const accountFile = (site, account) => join(siteDir(site), 'accounts', `${folderName(account)}.json`);
  const dayDir = (site, when, account) => join(siteDir(site), when, folderName(account));
  const postDir = (site, when, account, id) => join(dayDir(site, when, account), 'posts', folderName(id));
  const draftDir = (id) => join(root, 'drafts', folderName(id));

  const store = {
    root,
    postDir,
    draftDir,
    dayDir,

    /** An account's record: every post id ever collected (with the day it went in) and its runs. */
    async account(site, account) {
      const record = await readJson(accountFile(site, account), null);
      return (
        record ?? {
          site,
          account,
          addedAt: new Date().toISOString(),
          known: {},
          lastRun: null,
          runs: 0,
          gap: null,
        }
      );
    },
    async saveAccount(record) {
      await writeJson(accountFile(record.site, record.account), record);
    },
    /** Every account followed on `site`. */
    async accounts(site) {
      const dir = join(siteDir(site), 'accounts');
      const names = await readdir(dir).catch(() => []);
      const records = [];
      for (const name of names.filter((n) => n.endsWith('.json'))) {
        const record = await readJson(join(dir, name), null);
        if (record) records.push(record);
      }
      return records.sort((a, b) => a.account.localeCompare(b.account));
    },
    async removeAccount(site, account) {
      await rm(accountFile(site, account), { force: true });
    },

    async runs(site, when, account) {
      return readJson(join(dayDir(site, when, account), 'runs.json'), []);
    },
    async addRun(site, when, account, run) {
      const file = join(dayDir(site, when, account), 'runs.json');
      const runs = await readJson(file, []);
      runs.push(run);
      await writeJson(file, runs);
    },

    async post(site, when, account, id) {
      return readJson(join(postDir(site, when, account, id), 'post.json'), null);
    },
    async savePost(post) {
      await writeJson(join(postDir(post.site, post.day, post.account, post.id), 'post.json'), post);
    },
    async saveRaw(site, when, account, id, raw) {
      await writeJson(join(postDir(site, when, account, id), 'raw.json'), raw);
    },
    async saveMarkdown(post, markdown) {
      await writeText(join(postDir(post.site, post.day, post.account, post.id), 'post.md'), markdown);
    },
    /** Every post of an account that is on disk, newest first. */
    async posts(site, account) {
      const record = await store.account(site, account);
      const posts = [];
      for (const [id, entry] of Object.entries(record.known)) {
        const post = await store.post(site, entry.day, account, id);
        if (post) posts.push(post);
      }
      return posts.sort((a, b) => String(b.postedAt ?? '').localeCompare(String(a.postedAt ?? '')));
    },
    /** Changes a post's stage, in its file and in its account's record. */
    async setPostStatus(post, status, extra = {}) {
      if (!POST_STATUS.includes(status)) throw new Error(`unknown status ${status}`);
      const updated = { ...post, ...extra, status, statusAt: new Date().toISOString() };
      await store.savePost(updated);
      const record = await store.account(post.site, post.account);
      if (record.known[post.id]) {
        record.known[post.id].status = status;
        await store.saveAccount(record);
      }
      return updated;
    },

    async draft(id) {
      return readJson(join(draftDir(id), 'draft.json'), null);
    },
    async saveDraft(draft) {
      await writeJson(join(draftDir(draft.id), 'draft.json'), { ...draft, updatedAt: new Date().toISOString() });
    },
    async saveDraftMarkdown(draft, markdown) {
      await writeText(join(draftDir(draft.id), 'draft.md'), markdown);
    },
    async drafts() {
      const names = await readdir(join(root, 'drafts')).catch(() => []);
      const drafts = [];
      for (const name of names) {
        const draft = await readJson(join(root, 'drafts', name, 'draft.json'), null);
        if (draft) drafts.push(draft);
      }
      return drafts.sort((a, b) => String(b.createdAt).localeCompare(String(a.createdAt)));
    },
    /** Copies a post's media file into a draft, and says where it went (relative to the draft). */
    async copyToDraft(draft, post, relative) {
      const name = `${folderName(post.id)}_${relative.split('/').pop()}`;
      const kind = relative.startsWith('videos/') ? 'videos' : 'images';
      const target = join(draftDir(draft.id), 'media', kind, name);
      await mkdir(dirname(target), { recursive: true });
      await copyFile(join(postDir(post.site, post.day, post.account, post.id), relative), target);
      return `media/${kind}/${name}`;
    },

    /** An automation's settings (a tab of the plugin's workspace), by its id. */
    async engine(id) {
      return readJson(join(root, 'engines', `${folderName(id)}.json`), null);
    },
    async saveEngine(id, engine) {
      await writeJson(join(root, 'engines', `${folderName(id)}.json`), engine);
    },
    async removeEngine(id) {
      await rm(join(root, 'engines', `${folderName(id)}.json`), { force: true });
    },

    async styles(defaults) {
      const saved = await readJson(join(root, 'styles.json'), null);
      if (!saved?.styles?.length) return defaults;
      // A default style added by an update shows up next to the user's own.
      const ids = new Set(saved.styles.map((style) => style.id));
      return [...saved.styles, ...defaults.filter((style) => !ids.has(style.id))];
    },
    async saveStyles(styles) {
      await writeJson(join(root, 'styles.json'), { styles });
    },
  };
  return store;
}
