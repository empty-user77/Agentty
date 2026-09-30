// Cosmica for Agentty: bring Cosmica notes into agent prompts, and file session summaries back
// into Cosmica. Cosmica opens `agentty://plugin/cosmica/continue?path=…` for "Continue in Agentty".

import fs from 'node:fs/promises';
import { existsSync, statSync } from 'node:fs';
import path from 'node:path';
import { createPlugin, ui } from './agentty-plugin.mjs';
import * as cosmica from './cosmica.mjs';

const plugin = createPlugin();

const STRINGS = {
  en: {
    connected: 'Cosmica is running',
    offline: 'Cosmica is closed',
    notFound: 'Cosmica not found',
    notFoundBody: 'Install Cosmica (cosmica.app) or create notes in ~/CosmicaNotes/Note. This panel updates once notes exist.',
    saveTitle: 'Save this session',
    save: 'Save to Cosmica',
    logOnly: 'Log only',
    saveHint: 'The agent writes a summary note. "Log only" saves the conversation as it is, without AI.',
    noPane: 'Focus a Claude Code or Codex pane to save its session.',
    paneStatus: '{agent} · {status}',
    writing: 'Writing the summary…',
    continueTitle: 'Continue from a note',
    continueHint: 'Click a note to continue it in a new tab.',
    search: 'Search notes',
    noNotes: 'No notes found.',
    insert: 'Insert into the focused terminal',
    elsewhere: 'Continue elsewhere…',
    more: 'Show more',
    less: 'Show less',
    started: 'Continuing "{title}" in a new tab',
    typed: '"{title}" is in a new tab — press Enter to start.',
    needAgent: 'Focus a Claude Code or Codex pane first.',
    needsAnswer: 'The agent is waiting for your answer — answer it first, then save.',
    asked: 'Asked the agent to write a summary into Cosmica…',
    queued: 'The agent is working — it writes the summary as soon as it finishes.',
    pressEnter: 'The summary request is typed into the agent pane — press Enter there to start it.',
    saved: 'Saved to Cosmica: {title}',
    lastSaved: 'Saved',
    reveal: 'Show in Finder',
    folder: 'Folder',
    newFolder: 'New folder',
    newFolderName: 'New folder name',
    create: 'Create',
    folderSaved: 'Sessions will be saved to {folder}',
    refresh: 'Refresh',
    summaryTimeout: 'No summary file appeared yet. Check the agent pane.',
    continuePrompt: 'Continue working from the Cosmica note "{title}".\n(Source note: {path})\n\n---\n\n{body}',
    insertPrompt: 'Cosmica note "{title}" ({path}):\n\n{body}',
    summaryPrompt: [
      'Please write a summary of this session as a Cosmica note.',
      '',
      'Create exactly one new Markdown file at this path (create the folder if needed):',
      '{path}',
      '',
      'Use this format:',
      '---',
      'source: agentty',
      'tags: [#agentty, #session-summary]',
      'created_at: {date}',
      'cwd: {cwd}',
      'agent: {agent}',
      '---',
      '# <a short title for the work in this session>',
      '',
      '## Goal',
      '## What was done',
      '## Files changed',
      '## Decisions and findings',
      '## Next steps',
      '',
      'Write it in the language we have been using. Do not modify any other file and do not commit.',
    ].join('\n'),
    logTitle: '{title} — conversation log',
    user: 'User',
    assistant: 'Assistant',
    status: { idle: 'idle', working: 'working', thinking: 'thinking', finished: 'finished', permission: 'waiting for approval', question: 'waiting for an answer', interrupted: 'interrupted', exited: 'exited', shell: 'terminal' },
  },
  ko: {
    connected: 'Cosmica 실행 중',
    offline: 'Cosmica 꺼짐',
    notFound: 'Cosmica를 찾을 수 없음',
    notFoundBody: 'Cosmica를 설치하거나 ~/CosmicaNotes/Note 에 노트를 만들어 주세요. 노트가 생기면 이 패널이 갱신됩니다.',
    saveTitle: '이 세션 저장',
    save: 'Cosmica에 저장',
    logOnly: '대화 기록만',
    saveHint: '에이전트가 요약 노트를 작성합니다. "대화 기록만"은 AI 없이 대화를 그대로 저장합니다.',
    noPane: 'Claude Code 또는 Codex 창을 선택하면 세션을 저장할 수 있습니다.',
    paneStatus: '{agent} · {status}',
    writing: '요약 작성 중…',
    continueTitle: '노트에서 이어서 작업',
    continueHint: '노트를 누르면 새 탭에서 이어서 작업합니다.',
    search: '노트 검색',
    noNotes: '노트가 없습니다.',
    insert: '현재 터미널에 넣기',
    elsewhere: '다른 곳에서 이어가기…',
    more: '더 보기',
    less: '접기',
    started: '새 탭에서 "{title}" 이어서 작업합니다',
    typed: '새 탭에 "{title}"을(를) 넣었습니다. Enter를 누르면 시작합니다.',
    needAgent: '먼저 Claude Code 또는 Codex 창을 선택해 주세요.',
    needsAnswer: '에이전트가 답변을 기다리고 있습니다. 먼저 답한 뒤 저장해 주세요.',
    asked: '에이전트에게 Cosmica 요약 작성을 요청했습니다…',
    queued: '에이전트가 작업 중입니다. 끝나는 대로 요약을 작성합니다.',
    pressEnter: '에이전트 창에 요약 요청을 입력해 두었습니다. 그 창에서 Enter를 누르면 시작합니다.',
    saved: 'Cosmica에 저장됨: {title}',
    lastSaved: '저장됨',
    reveal: 'Finder에서 보기',
    folder: '저장 폴더',
    newFolder: '새 폴더',
    newFolderName: '새 폴더 이름',
    create: '만들기',
    folderSaved: '세션은 {folder} 폴더에 저장됩니다',
    refresh: '새로고침',
    summaryTimeout: '아직 요약 파일이 만들어지지 않았습니다. 에이전트 창을 확인해 주세요.',
    logTitle: '{title} — 대화 기록',
    user: '사용자',
    assistant: '어시스턴트',
    status: { idle: '대기', working: '작업 중', thinking: '생각 중', finished: '완료', permission: '승인 대기', question: '답변 대기', interrupted: '중단됨', exited: '종료됨', shell: '터미널' },
  },
  ja: {
    connected: 'Cosmica 起動中',
    offline: 'Cosmica は終了しています',
    notFound: 'Cosmica が見つかりません',
    notFoundBody: 'Cosmica をインストールするか ~/CosmicaNotes/Note にノートを作成してください。',
    saveTitle: 'このセッションを保存',
    save: 'Cosmica に保存',
    logOnly: '会話ログのみ',
    saveHint: 'エージェントが要約ノートを作成します。「会話ログのみ」は AI を使わず会話をそのまま保存します。',
    noPane: 'Claude Code か Codex のペインを選択するとセッションを保存できます。',
    paneStatus: '{agent} · {status}',
    writing: '要約を作成中…',
    continueTitle: 'ノートから続ける',
    continueHint: 'ノートをクリックすると新しいタブで続きを作業します。',
    search: 'ノートを検索',
    noNotes: 'ノートがありません。',
    insert: 'フォーカス中のターミナルに挿入',
    elsewhere: '別の場所で続ける…',
    more: 'もっと見る',
    less: '折りたたむ',
    started: '新しいタブで「{title}」の続きを作業します',
    typed: '新しいタブに「{title}」を入力しました。Enter で開始します。',
    needAgent: '先に Claude Code か Codex のペインを選択してください。',
    needsAnswer: 'エージェントが回答を待っています。先に回答してから保存してください。',
    asked: 'エージェントに Cosmica への要約作成を依頼しました…',
    queued: 'エージェントが作業中です。終わり次第、要約を作成します。',
    pressEnter: 'エージェントのペインに要約の依頼を入力しました。そのペインで Enter を押すと開始します。',
    saved: 'Cosmica に保存しました: {title}',
    lastSaved: '保存済み',
    reveal: 'Finder で表示',
    folder: '保存先フォルダ',
    newFolder: '新しいフォルダ',
    newFolderName: '新しいフォルダ名',
    create: '作成',
    folderSaved: 'セッションは {folder} に保存されます',
    refresh: '更新',
    summaryTimeout: '要約ファイルがまだ作成されていません。エージェントのペインを確認してください。',
    logTitle: '{title} — 会話ログ',
    user: 'ユーザー',
    assistant: 'アシスタント',
    status: { idle: '待機中', working: '作業中', thinking: '思考中', finished: '完了', permission: '承認待ち', question: '回答待ち', interrupted: '中断', exited: '終了', shell: 'ターミナル' },
  },
  zh: {
    connected: 'Cosmica 运行中',
    offline: 'Cosmica 未运行',
    notFound: '未找到 Cosmica',
    notFoundBody: '请安装 Cosmica，或在 ~/CosmicaNotes/Note 中创建笔记。',
    saveTitle: '保存此会话',
    save: '保存到 Cosmica',
    logOnly: '仅对话记录',
    saveHint: '智能体会写一篇摘要笔记。“仅对话记录”不使用 AI，按原样保存对话。',
    noPane: '选择 Claude Code 或 Codex 窗格即可保存其会话。',
    paneStatus: '{agent} · {status}',
    writing: '正在写摘要…',
    continueTitle: '从笔记继续',
    continueHint: '点击笔记即可在新标签页中继续工作。',
    search: '搜索笔记',
    noNotes: '没有笔记。',
    insert: '插入到当前终端',
    elsewhere: '在其他位置继续…',
    more: '显示更多',
    less: '收起',
    started: '在新标签页中继续“{title}”',
    typed: '已将“{title}”放入新标签页，按 Enter 开始。',
    needAgent: '请先选择 Claude Code 或 Codex 窗格。',
    needsAnswer: '智能体正在等待你的回答，请先回答后再保存。',
    asked: '已请智能体将摘要写入 Cosmica…',
    queued: '智能体正在工作，完成后会立即写摘要。',
    pressEnter: '已在智能体窗格中输入摘要请求，在该窗格按 Enter 即可开始。',
    saved: '已保存到 Cosmica：{title}',
    lastSaved: '已保存',
    reveal: '在访达中显示',
    folder: '保存文件夹',
    newFolder: '新建文件夹',
    newFolderName: '新文件夹名称',
    create: '创建',
    folderSaved: '会话将保存到 {folder}',
    refresh: '刷新',
    summaryTimeout: '摘要文件尚未生成，请查看智能体窗格。',
    logTitle: '{title} — 对话记录',
    user: '用户',
    assistant: '助手',
    status: { idle: '空闲', working: '工作中', thinking: '思考中', finished: '已完成', permission: '等待批准', question: '等待回答', interrupted: '已中断', exited: '已退出', shell: '终端' },
  },
};

const AGENT_NAMES = { claude: 'Claude Code', codex: 'Codex' };

/** Notes shown before "Show more", and with it (or while searching). */
const FEW_NOTES = 6;
const MANY_NOTES = 40;

const state = {
  config: null,
  running: false,
  query: '',
  notes: [],
  showAll: false,
  /** Folders of the notes folder, offered as places to save sessions. */
  folders: [],
  /** The "new folder" field is shown. */
  naming: false,
  folderName: '',
  settings: { folder: 'Agentty' },
  /** The summary file an agent is writing now. */
  writing: null,
  lastSaved: null,
  panelOpen: false,
};

function language() {
  const code = plugin.context?.language ?? plugin.info?.language ?? 'en';
  return STRINGS[code] ? code : 'en';
}

function tr(key, values = {}) {
  const table = STRINGS[language()];
  let text = table[key] ?? STRINGS.en[key] ?? key;
  for (const [name, value] of Object.entries(values)) text = text.split(`{${name}}`).join(String(value));
  return text;
}

function relativeTime(ms) {
  const minutes = Math.round((Date.now() - ms) / 60000);
  const rtf = new Intl.RelativeTimeFormat(language(), { numeric: 'auto' });
  if (minutes < 60) return rtf.format(-minutes, 'minute');
  if (minutes < 60 * 24) return rtf.format(-Math.round(minutes / 60), 'hour');
  return rtf.format(-Math.round(minutes / 1440), 'day');
}

function isDir(dir) {
  try {
    return typeof dir === 'string' && path.isAbsolute(dir) && statSync(dir).isDirectory();
  } catch {
    return false;
  }
}

// -- settings -------------------------------------------------------------------------------------

function settingsFile() {
  return path.join(plugin.info?.plugin.dataDir ?? '.', 'settings.json');
}

async function loadSettings() {
  try {
    state.settings = { ...state.settings, ...JSON.parse(await fs.readFile(settingsFile(), 'utf8')) };
  } catch {
    // First run.
  }
}

async function saveSettings() {
  await fs.mkdir(path.dirname(settingsFile()), { recursive: true });
  await fs.writeFile(settingsFile(), JSON.stringify(state.settings, null, 2));
}

// -- notes ----------------------------------------------------------------------------------------

async function refresh() {
  state.config = await cosmica.loadConfig();
  state.running = await cosmica.cosmicaRunning(state.config);
  if (!state.config.notesExist) {
    state.notes = [];
    state.folders = [];
    return;
  }
  state.folders = await cosmica.listFolders(state.config.notesPath);
  // One more than shown tells whether "Show more" has anything to show.
  const limit = state.query || state.showAll ? MANY_NOTES : FEW_NOTES + 1;
  state.notes = state.query ? await cosmica.searchNotes(state.config.notesPath, state.query, limit) : await cosmica.recentNotes(state.config.notesPath, limit);
}

function noteLabel(note) {
  return note.folder ? `${note.folder} · ${relativeTime(note.mtimeMs)}` : relativeTime(note.mtimeMs);
}

async function notePrompt(key, notePath) {
  const note = await cosmica.readNote(state.config.notesPath, notePath);
  return { note, text: tr(key, { title: note.title, path: note.path, body: note.body }) };
}

/**
 * Continues a note in a new agent tab, without asking where: the folder the note was written in
 * (notes Agentty saved record it), else the focused pane's or workspace's folder, with the agent
 * that wrote it or the focused one. `pick` shows the "Send to…" dialog instead. A link from
 * Cosmica (any app or page can open one) only types the prompt; the user presses Enter.
 */
async function continueNote(notePath, context, { pick = false, fromLink = false } = {}) {
  if (!state.config) state.config = await cosmica.loadConfig();
  const { note, text } = await notePrompt('continuePrompt', notePath);
  const cwd = [note.frontmatter.cwd, context?.pane?.cwd, context?.workspace?.cwd].find(isDir);
  if (pick) return plugin.injectPrompt({ text, title: note.title, target: 'ask', cwd });
  const agent = [note.frontmatter.agent, context?.pane?.kind].find((kind) => kind in AGENT_NAMES) ?? 'claude';
  const result = await plugin.injectPrompt({ text, title: note.title, target: context?.workspace ? 'newTab' : 'newWorkspace', cwd, agent, submit: !fromLink });
  // Agentty leaves Enter to the user once a link reached the plugin, and may ask where it goes.
  if (result?.status === 'sent') await plugin.notify(tr(result.submitted === false ? 'typed' : 'started', { title: note.title }), 'info');
}

async function insertNote(notePath) {
  const { text } = await notePrompt('insertPrompt', notePath);
  await plugin.injectPrompt({ text, target: 'active', submit: false });
}

// -- sessions -------------------------------------------------------------------------------------

function focusedAgent(context) {
  const pane = context?.pane;
  return pane && pane.kind !== 'shell' && pane.running ? pane : null;
}

async function saveAiSummary(context) {
  const pane = focusedAgent(context);
  if (!pane) return plugin.notify(tr('needAgent'), 'warning');
  if (['permission', 'question'].includes(pane.status)) return plugin.notify(tr('needsAnswer'), 'warning');
  // A working agent queues the request and writes the summary once its current turn ends.
  const busy = ['working', 'thinking'].includes(pane.status);
  if (!state.config) state.config = await cosmica.loadConfig();
  // The folder is made here (the default Agentty one on first use), not left to the agent.
  const folder = await cosmica.createFolder(state.config.notesPath, state.settings.folder);
  const target = path.join(state.config.notesPath, folder, cosmica.newNoteName());
  // Prompts are English in every UI language; this one asks for the note in the language in use.
  const text = STRINGS.en.summaryPrompt
    .split('{path}')
    .join(target)
    .split('{date}')
    .join(new Date().toISOString())
    .split('{cwd}')
    .join(pane.cwd)
    .split('{agent}')
    .join(pane.kind);
  const sent = await plugin.sendToTerminal({ paneId: pane.id, text, submit: true });
  // After a Cosmica link, Agentty types the request but leaves Enter to the user: nothing is being
  // written until they press it, so the button stays usable and the file is watched quietly.
  const waiting = sent?.submitted === false;
  await plugin.notify(tr(waiting ? 'pressEnter' : busy ? 'queued' : 'asked'), 'info');
  if (!waiting) {
    state.writing = target;
    await render();
  }
  watchForSummary(target, busy ? 45 : 15, { quiet: waiting });
}

/** Waits for the agent to write the summary, then tells Cosmica about it. */
async function watchForSummary(target, minutes, { quiet = false } = {}) {
  const started = Date.now();
  let lastSize = -1;
  try {
    while (Date.now() - started < minutes * 60 * 1000) {
      await new Promise((resolve) => setTimeout(resolve, 2000));
      let size;
      try {
        size = (await fs.stat(target)).size;
      } catch {
        continue;
      }
      // Written and no longer growing.
      if (size > 0 && size === lastSize) {
        const { title } = cosmica.parseNote(await fs.readFile(target, 'utf8'));
        await cosmica.refreshCosmica(state.config);
        state.lastSaved = { path: target, title: title || path.basename(target) };
        await plugin.notify(tr('saved', { title: state.lastSaved.title }), 'success');
        if (state.panelOpen) await refresh();
        return;
      }
      lastSize = size;
    }
    if (!quiet) await plugin.notify(tr('summaryTimeout'), 'warning');
  } finally {
    if (state.writing === target) state.writing = null;
    await render();
  }
}

async function saveTranscript(context) {
  const pane = focusedAgent(context);
  if (!pane) return plugin.notify(tr('needAgent'), 'warning');
  if (!state.config) state.config = await cosmica.loadConfig();
  const session = await plugin.getSession({ paneId: pane.id, maxTurns: 400 });
  const parts = [
    `- ${AGENT_NAMES[session.agent] ?? session.agent} · \`${session.sessionId}\``,
    session.cwd ? `- \`${session.cwd}\`` : null,
    `- ${session.turnCount} turns`,
    '',
  ].filter((line) => line !== null);
  let budget = 200_000;
  session.turns.forEach((turn, index) => {
    if (budget <= 0) return;
    const text = turn.text.length > budget ? `${turn.text.slice(0, budget)}…` : turn.text;
    budget -= text.length;
    parts.push(`## ${index + 1}. ${turn.role === 'user' ? tr('user') : tr('assistant')}`, '', text, '');
  });
  const title = tr('logTitle', { title: session.title || pane.title });
  const saved = await cosmica.writeNote(state.config, {
    folder: state.settings.folder,
    title,
    body: parts.join('\n'),
    tags: ['agentty', 'session-log'],
    extra: { agent: session.agent, session_id: session.sessionId, cwd: session.cwd },
  });
  state.lastSaved = { path: saved.path, title };
  await plugin.notify(tr('saved', { title }), 'success');
  if (state.panelOpen) await refresh();
  await render();
}

// -- panel ----------------------------------------------------------------------------------------

/** Cosmica's folders, the default Agentty one (made when first saved to) and "New folder". */
function folderOptions() {
  const names = [...new Set(['Agentty', state.settings.folder, ...state.folders])];
  return [...names.map((name) => ({ value: name, label: name })), { value: NEW_FOLDER, label: `+ ${tr('newFolder')}` }];
}

// A folder name cannot hold ':' on macOS, so this never clashes with a real folder.
const NEW_FOLDER = ':new';

function saveSection(pane) {
  const status = pane ? STRINGS[language()].status[pane.status] ?? pane.status : '';
  return ui.section(tr('saveTitle'), [
    ui.text(pane ? tr('paneStatus', { agent: AGENT_NAMES[pane.kind] ?? pane.kind, status }) : tr('noPane'), 'muted'),
    ui.row(
      [
        ui.button('save', tr('save'), { icon: 'save', variant: 'primary', disabled: !pane || Boolean(state.writing) }),
        ui.button('transcript', tr('logOnly'), { icon: 'scroll-text', variant: 'ghost', disabled: !pane }),
      ],
      { wrap: true, gap: 'small' },
    ),
    ui.text(tr('folder'), 'small'),
    ui.choice('folder', folderOptions(), state.settings.folder),
    state.naming &&
      ui.row([ui.input('new-folder', { placeholder: tr('newFolderName') }), ui.button('create-folder', tr('create'), { icon: 'folder-plus' })], { gap: 'small' }),
    state.writing
      ? ui.spinner(tr('writing'))
      : state.lastSaved
        ? ui.row([ui.text(`${tr('lastSaved')}: ${state.lastSaved.title}`, 'small'), ui.button('reveal', tr('reveal'), { icon: 'folder-open', variant: 'ghost' })], { gap: 'small', wrap: true })
        : ui.text(tr('saveHint'), 'small'),
  ]);
}

function notesSection() {
  const shown = state.query || state.showAll ? state.notes : state.notes.slice(0, FEW_NOTES);
  const items = shown.map((note) => ({
    id: note.path,
    title: note.title,
    subtitle: noteLabel(note),
    icon: note.locked ? 'shield-alert' : note.source === 'agentty' ? 'bot' : 'file-text',
    actions: note.locked
      ? []
      : [
          { id: 'insert', icon: 'file-input', tooltip: tr('insert') },
          { id: 'elsewhere', icon: 'ellipsis', tooltip: tr('elsewhere') },
        ],
  }));
  const hasMore = !state.query && !state.showAll && state.notes.length > FEW_NOTES;
  return ui.section(tr('continueTitle'), [
    ui.input('query', { placeholder: tr('search'), value: state.query }),
    ui.list('notes', items, { empty: tr('noNotes') }),
    hasMore && ui.button('more', tr('more'), { icon: 'chevron-down', variant: 'ghost' }),
    !state.query && state.showAll && ui.button('less', tr('less'), { icon: 'chevron-up', variant: 'ghost' }),
    ui.text(tr('continueHint'), 'small'),
  ]);
}

function footer(config) {
  const status = !config.notesExist ? tr('notFound') : state.running ? tr('connected') : tr('offline');
  return ui.column(
    [
      ui.divider(),
      ui.row(
        [
          ui.badge(status, !config.notesExist ? 'error' : state.running ? 'success' : 'neutral'),
          ui.button('refresh', tr('refresh'), { icon: 'refresh-cw', variant: 'ghost' }),
        ],
        { gap: 'small', wrap: true },
      ),
      ui.text(config.notesPath, 'small'),
    ],
    { gap: 'small' },
  );
}

async function render() {
  if (!state.panelOpen) return;
  const config = state.config ?? (await cosmica.loadConfig());
  if (!config.notesExist) {
    return plugin.setPanel(ui.column([ui.text(tr('notFoundBody'), 'muted'), footer(config)]));
  }
  await plugin.setPanel(ui.column([saveSection(focusedAgent(plugin.context)), notesSection(), footer(config)]));
}

async function chooseFolder(folder) {
  state.settings.folder = folder;
  await saveSettings();
  await render();
}

async function createFolder(name) {
  if (!String(name ?? '').trim()) return;
  if (!state.config) state.config = await cosmica.loadConfig();
  const folder = await cosmica.createFolder(state.config.notesPath, name);
  state.naming = false;
  state.folderName = '';
  state.folders = await cosmica.listFolders(state.config.notesPath);
  await plugin.notify(tr('folderSaved', { folder }), 'success');
  await chooseFolder(folder);
}

async function refreshAndRender() {
  await refresh();
  await render();
}

let searchTimer = null;

plugin
  .onActivate(async () => {
    await loadSettings();
    state.config = await cosmica.loadConfig();
  })
  .onPanelOpen(async () => {
    state.panelOpen = true;
    await refreshAndRender();
  })
  .onPanelClose(() => {
    state.panelOpen = false;
  })
  .onContextChange(() => render())
  .onEvent('refresh', refreshAndRender)
  .onEvent('more', async () => {
    state.showAll = true;
    await refreshAndRender();
  })
  .onEvent('less', async () => {
    state.showAll = false;
    await refreshAndRender();
  })
  .onEvent('query', async (event) => {
    state.query = String(event.value ?? '');
    clearTimeout(searchTimer);
    searchTimer = setTimeout(refreshAndRender, event.event === 'submit' ? 0 : 200);
  })
  .onEvent('notes', async (event, context) => {
    if (event.event === 'action' && event.action === 'insert') return insertNote(event.item);
    if (event.event === 'action' && event.action === 'elsewhere') return continueNote(event.item, context, { pick: true });
    // Clicking a row continues the note in a new tab.
    return continueNote(event.item, context);
  })
  .onEvent('save', (_event, context) => saveAiSummary(context))
  .onEvent('transcript', (_event, context) => saveTranscript(context))
  .onEvent('reveal', () => state.lastSaved && existsSync(state.lastSaved.path) && plugin.revealPath(state.lastSaved.path))
  .onEvent('folder', async (event) => {
    if (event.value === NEW_FOLDER) {
      state.naming = true;
      return render();
    }
    state.naming = false;
    await chooseFolder(cosmica.safeFolder(event.value));
  })
  .onEvent('new-folder', async (event) => {
    state.folderName = String(event.value ?? '');
    if (event.event === 'submit') await createFolder(state.folderName);
  })
  // The field's text arrives (as a change) before the button's click.
  .onEvent('create-folder', () => createFolder(state.folderName))
  .command('cosmica.saveSummary', ({ context }) => saveAiSummary(context))
  .command('cosmica.saveTranscript', ({ context }) => saveTranscript(context))
  .command('cosmica.browse', () => plugin.showPanel())
  .command('cosmica.continueLatest', async ({ context }) => {
    state.config = await cosmica.loadConfig();
    const [latest] = await cosmica.recentNotes(state.config.notesPath, 1);
    if (!latest) return plugin.notify(tr('noNotes'), 'warning');
    return continueNote(latest.path, context);
  })
  // "Continue in Agentty" from Cosmica.
  .onUrl('continue', async ({ query }) => {
    state.config = await cosmica.loadConfig();
    await continueNote(query.path, plugin.context, { fromLink: true });
  })
  .start();
