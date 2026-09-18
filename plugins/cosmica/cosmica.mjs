// Reading and writing Cosmica notes. Cosmica keeps notes as Markdown files (the source of truth)
// under `notes.path` from ~/.cosmica/config.json, and runs a local API on 127.0.0.1 while open.
// Only the notes path and server port are read from the config — never keys or tokens.

import fs from 'node:fs/promises';
import { existsSync, realpathSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const MAX_NOTE_BYTES = 1024 * 1024;
const WALK_DEPTH = 6;

export function cosmicaHome() {
  return process.env.COSMICA_HOME || path.join(os.homedir(), '.cosmica');
}

/** { configFound, appInstalled, notesPath, port, notesExist } */
export async function loadConfig() {
  const configPath = path.join(cosmicaHome(), 'config.json');
  let raw = null;
  try {
    raw = JSON.parse(await fs.readFile(configPath, 'utf8'));
  } catch {
    raw = null;
  }
  const configured = typeof raw?.notes?.path === 'string' && raw.notes.path ? raw.notes.path : path.join(os.homedir(), 'CosmicaNotes', 'Note');
  const notesPath = configured.startsWith('~') ? path.join(os.homedir(), configured.slice(1)) : configured;
  let port = Number(raw?.server?.port) || 5777;
  try {
    port = Number((await fs.readFile(path.join(cosmicaHome(), 'port'), 'utf8')).trim()) || port;
  } catch {
    // Not running (or an older version): keep the configured port.
  }
  const appInstalled = ['/Applications/Cosmica.app', path.join(os.homedir(), 'Applications', 'Cosmica.app')].some((p) => existsSync(p));
  return { configFound: raw !== null, appInstalled, notesPath, port, notesExist: existsSync(notesPath), configPath };
}

/** Splits front matter, title (first `# ` heading, else first line) and body. */
export function parseNote(text) {
  let body = text.replace(/^﻿/, '');
  const frontmatter = {};
  const match = body.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/);
  if (match) {
    for (const line of match[1].split(/\r?\n/)) {
      const pair = line.match(/^([A-Za-z0-9_-]+):\s*(.*)$/);
      if (pair) frontmatter[pair[1]] = pair[2].trim();
    }
    body = body.slice(match[0].length);
  }
  const lines = body.split(/\r?\n/);
  const headingIndex = lines.findIndex((l) => /^#\s+\S/.test(l));
  const firstIndex = lines.findIndex((l) => l.trim() !== '');
  let title = '';
  if (headingIndex !== -1 && headingIndex === firstIndex) {
    title = lines[headingIndex].replace(/^#\s+/, '').trim();
  } else if (firstIndex !== -1) {
    title = lines[firstIndex].replace(/^#+\s*/, '').trim().slice(0, 80);
  }
  const locked = /^(true|yes)$/i.test(frontmatter.locked ?? '') || /^(true|yes)$/i.test(frontmatter.encrypted ?? '');
  return { title, body: body.trim(), frontmatter, locked };
}

/** Markdown files under `root`, newest first: { path, relPath, folder, mtimeMs, size }. */
export async function listNoteFiles(root, limit = 500) {
  const files = [];
  async function walk(dir, depth) {
    let entries;
    try {
      entries = await fs.readdir(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (entry.name.startsWith('.')) continue;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (depth < WALK_DEPTH) await walk(full, depth + 1);
      } else if (entry.isFile() && entry.name.toLowerCase().endsWith('.md')) {
        try {
          const stat = await fs.stat(full);
          const relPath = path.relative(root, full);
          files.push({ path: full, relPath, folder: path.dirname(relPath) === '.' ? '' : path.dirname(relPath), mtimeMs: stat.mtimeMs, size: stat.size });
        } catch {
          // Deleted while walking.
        }
      }
    }
  }
  await walk(root, 0);
  files.sort((a, b) => b.mtimeMs - a.mtimeMs);
  return files.slice(0, limit);
}

/** Recent notes with titles (reads the start of each file). */
export async function recentNotes(root, limit = 30) {
  const files = await listNoteFiles(root, limit);
  return Promise.all(files.map(async (file) => ({ ...file, ...(await peek(file.path)) })));
}

async function peek(file) {
  try {
    const handle = await fs.open(file, 'r');
    try {
      const buffer = Buffer.alloc(4096);
      const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0);
      const { title, locked, frontmatter } = parseNote(buffer.subarray(0, bytesRead).toString('utf8'));
      return { title: title || path.basename(file, '.md'), locked, source: frontmatter.source ?? '' };
    } finally {
      await handle.close();
    }
  } catch {
    return { title: path.basename(file, '.md'), locked: false, source: '' };
  }
}

/** Notes whose title or text contains every word of `query` (case-insensitive). */
export async function searchNotes(root, query, limit = 30) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return recentNotes(root, limit);
  const results = [];
  for (const file of await listNoteFiles(root, 2000)) {
    if (file.size > MAX_NOTE_BYTES) continue;
    let text;
    try {
      text = await fs.readFile(file.path, 'utf8');
    } catch {
      continue;
    }
    const note = parseNote(text);
    if (note.locked) continue;
    const haystack = `${note.title}\n${note.body}`.toLowerCase();
    if (words.every((w) => haystack.includes(w))) {
      results.push({ ...file, title: note.title || path.basename(file.path, '.md'), locked: false, source: note.frontmatter.source ?? '' });
      if (results.length >= limit) break;
    }
  }
  return results;
}

/** Resolves a note path and makes sure it is a Markdown file inside the notes folder. */
export function resolveNotePath(root, notePath) {
  if (typeof notePath !== 'string' || !notePath) throw new Error('no note path');
  const candidate = path.isAbsolute(notePath) ? notePath : path.join(root, notePath);
  if (!candidate.toLowerCase().endsWith('.md')) throw new Error('not a Markdown note');
  let realRoot;
  let realFile;
  try {
    realRoot = realpathSync(root);
    realFile = realpathSync(candidate);
  } catch {
    throw new Error(`note not found: ${notePath}`);
  }
  if (!realFile.startsWith(realRoot + path.sep)) throw new Error('the note is outside the Cosmica notes folder');
  return realFile;
}

export async function readNote(root, notePath) {
  const file = resolveNotePath(root, notePath);
  const stat = await fs.stat(file);
  if (stat.size > MAX_NOTE_BYTES) throw new Error('the note is larger than 1 MB');
  const note = parseNote(await fs.readFile(file, 'utf8'));
  if (note.locked) throw new Error('the note is locked in Cosmica');
  return { ...note, path: file, relPath: path.relative(realpathSync(root), file), title: note.title || path.basename(file, '.md') };
}

/** Cosmica-style file name: `YYYYMMDD-HHMMSS-xxxx.md`. */
export function newNoteName(date = new Date()) {
  const pad = (n) => String(n).padStart(2, '0');
  const stamp = `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(date.getHours())}${pad(date.getMinutes())}${pad(date.getSeconds())}`;
  return `${stamp}-${Math.random().toString(36).slice(2, 6).padEnd(4, '0')}.md`;
}

/** A folder name under the notes folder (no traversal). */
export function safeFolder(folder) {
  const clean = String(folder ?? '')
    .split(/[\\/]+/)
    .map((part) => part.trim())
    .filter((part) => part && part !== '.' && part !== '..' && !part.startsWith('.'))
    .join(path.sep);
  return clean || 'Agentty';
}

export function noteContent({ title, body, tags = ['agentty'], extra = {} }) {
  const lines = ['---', 'source: agentty', `tags: [${tags.map((t) => `#${t}`).join(', ')}]`, `created_at: ${new Date().toISOString()}`];
  for (const [key, value] of Object.entries(extra)) {
    if (value !== undefined && value !== null && value !== '') lines.push(`${key}: ${String(value).replace(/\r?\n/g, ' ')}`);
  }
  lines.push('---', `# ${title.replace(/\r?\n/g, ' ').trim() || 'Agentty'}`, '', body.trim(), '');
  return lines.join('\n');
}

async function api(config, method, route, body, timeoutMs = 2500) {
  const response = await fetch(`http://127.0.0.1:${config.port}${route}`, {
    method,
    headers: body ? { 'Content-Type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(timeoutMs),
  });
  if (!response.ok) throw new Error(`Cosmica API ${route}: HTTP ${response.status}`);
  return response.json();
}

export async function cosmicaRunning(config) {
  try {
    await api(config, 'GET', '/api/health', undefined, 800);
    return true;
  } catch {
    return false;
  }
}

/**
 * Saves a note: through Cosmica's API while it runs (indexed and synced right away), otherwise
 * as a file Cosmica picks up the next time it lists notes. Returns { path, via }.
 */
export async function writeNote(config, { folder, title, body, tags, extra }) {
  const content = noteContent({ title, body, tags, extra });
  const dir = safeFolder(folder);
  try {
    const meta = await api(config, 'POST', '/api/notes', { folder: dir, content });
    if (meta?.relativePath) return { path: path.join(config.notesPath, meta.relativePath), via: 'api' };
  } catch {
    // Cosmica is closed or refused; write the file directly.
  }
  const target = path.join(config.notesPath, dir, newNoteName());
  await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, content, { flag: 'wx' });
  return { path: target, via: 'file' };
}

/** Asks a running Cosmica to re-read the notes folder (picks up files written by agents). */
export async function refreshCosmica(config) {
  try {
    await api(config, 'GET', '/api/notes', undefined, 4000);
  } catch {
    // Not running: it reconciles on its next start.
  }
}
