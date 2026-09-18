// Per-project state remembered across panel opens: `<dataDir>/projects.json`, keyed by the
// project's absolute path. Holds only what's needed to show "Launched!" again without redoing work.

import fs from 'node:fs/promises';
import path from 'node:path';

function file(dataDir) {
  return path.join(dataDir, 'projects.json');
}

export async function loadAllProjects(dataDir) {
  try {
    return JSON.parse(await fs.readFile(file(dataDir), 'utf8'));
  } catch {
    return {};
  }
}

export async function loadProject(dataDir, root) {
  const all = await loadAllProjects(dataDir);
  return all[root] ?? null;
}

export async function saveProject(dataDir, root, patch) {
  const all = await loadAllProjects(dataDir);
  all[root] = { ...(all[root] ?? {}), ...patch };
  await fs.mkdir(dataDir, { recursive: true });
  await fs.writeFile(file(dataDir), JSON.stringify(all, null, 2));
  return all[root];
}
