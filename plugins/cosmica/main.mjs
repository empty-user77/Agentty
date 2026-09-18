// Cosmica for Agentty: bring Cosmica notes into agent prompts, and file session summaries back
// into Cosmica. Cosmica opens `agentty://plugin/cosmica/continue?path=…` for "Continue in Agentty".

import fs from 'node:fs/promises';
import path from 'node:path';
import { createPlugin, ui } from './agentty-plugin.mjs';
import * as cosmica from './cosmica.mjs';

const plugin = createPlugin();

const STRINGS = {
  en: {
    connected: 'Connected',
    offline: 'Cosmica is closed',
    notFound: 'Cosmica not found',
    notFoundBody: 'Install Cosmica (cosmica.app) or create notes in ~/CosmicaNotes/Note. This panel updates once notes exist.',
    notes: 'Notes → prompt',
    search: 'Search notes',
    noNotes: 'No notes found.',
    insert: 'Insert into the focused terminal',
    continueIn: 'Continue in…',
    session: 'Session → Cosmica',
    noPane: 'Focus a Claude Code or Codex pane to save its session.',
    paneStatus: '{agent} · {status}',
    aiSummary: 'Save AI summary',
    transcript: 'Save conversation log',
    busy: 'The agent is working — try again when it finishes.',
    needAgent: 'Focus a Claude Code or Codex pane first.',
    asked: 'Asked the agent to write a summary into Cosmica…',
    saved: 'Saved to Cosmica: {title}',
    lastSaved: 'Last saved',
    reveal: 'Show in Finder',
    settings: 'Settings',
    folder: 'Cosmica folder for sessions',
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
    connected: '연결됨',
    offline: 'Cosmica 꺼짐',
    notFound: 'Cosmica를 찾을 수 없음',
    notFoundBody: 'Cosmica를 설치하거나 ~/CosmicaNotes/Note 에 노트를 만들어 주세요. 노트가 생기면 이 패널이 갱신됩니다.',
    notes: '노트 → 프롬프트',
    search: '노트 검색',
    noNotes: '노트가 없습니다.',
    insert: '현재 터미널에 넣기',
    continueIn: '이어서 작업하기…',
    session: '세션 → Cosmica',
    noPane: 'Claude Code 또는 Codex 창을 선택하면 세션을 저장할 수 있습니다.',
    paneStatus: '{agent} · {status}',
    aiSummary: 'AI 요약 저장',
    transcript: '대화 기록 저장',
    busy: '에이전트가 작업 중입니다. 끝난 뒤 다시 시도해 주세요.',
    needAgent: '먼저 Claude Code 또는 Codex 창을 선택해 주세요.',
    asked: '에이전트에게 Cosmica 요약 작성을 요청했습니다…',
    saved: 'Cosmica에 저장됨: {title}',
    lastSaved: '마지막 저장',
    reveal: 'Finder에서 보기',
    settings: '설정',
    folder: '세션을 저장할 Cosmica 폴더',
    folderSaved: '세션은 {folder} 폴더에 저장됩니다',
    refresh: '새로고침',
    summaryTimeout: '아직 요약 파일이 만들어지지 않았습니다. 에이전트 창을 확인해 주세요.',
    continuePrompt: 'Cosmica 노트 "{title}"의 내용을 이어서 작업해 주세요.\n(원본 노트: {path})\n\n---\n\n{body}',
    insertPrompt: 'Cosmica 노트 "{title}" ({path}):\n\n{body}',
    summaryPrompt: [
      '이 세션에서 한 작업을 Cosmica 노트로 요약해 주세요.',
      '',
      '아래 경로에 새 Markdown 파일 하나만 만들어 주세요 (폴더가 없으면 만들기):',
      '{path}',
      '',
      '형식:',
      '---',
      'source: agentty',
      'tags: [#agentty, #session-summary]',
      'created_at: {date}',
      'cwd: {cwd}',
      '---',
      '# <이 세션 작업을 나타내는 짧은 제목>',
      '',
      '## 목표',
      '## 한 일',
      '## 변경한 파일',
      '## 결정 사항 · 알게 된 점',
      '## 다음 할 일',
      '',
      '지금까지 대화한 언어로 작성하고, 다른 파일은 수정하거나 커밋하지 마세요.',
    ].join('\n'),
    logTitle: '{title} — 대화 기록',
    user: '사용자',
    assistant: '어시스턴트',
    status: { idle: '대기', working: '작업 중', thinking: '생각 중', finished: '완료', permission: '승인 대기', question: '답변 대기', interrupted: '중단됨', exited: '종료됨', shell: '터미널' },
  },
  ja: {
    connected: '接続済み',
    offline: 'Cosmica は終了しています',
    notFound: 'Cosmica が見つかりません',
    notFoundBody: 'Cosmica をインストールするか ~/CosmicaNotes/Note にノートを作成してください。',
    notes: 'ノート → プロンプト',
    search: 'ノートを検索',
    noNotes: 'ノートがありません。',
    insert: 'フォーカス中のターミナルに挿入',
    continueIn: '続きを作業…',
    session: 'セッション → Cosmica',
    noPane: 'Claude Code か Codex のペインを選択するとセッションを保存できます。',
    paneStatus: '{agent} · {status}',
    aiSummary: 'AI 要約を保存',
    transcript: '会話ログを保存',
    busy: 'エージェントが作業中です。終わってから再試行してください。',
    needAgent: '先に Claude Code か Codex のペインを選択してください。',
    asked: 'エージェントに Cosmica への要約作成を依頼しました…',
    saved: 'Cosmica に保存しました: {title}',
    lastSaved: '最後に保存',
    reveal: 'Finder で表示',
    settings: '設定',
    folder: 'セッションを保存する Cosmica フォルダ',
    folderSaved: 'セッションは {folder} に保存されます',
    refresh: '更新',
    summaryTimeout: '要約ファイルがまだ作成されていません。エージェントのペインを確認してください。',
    continuePrompt: 'Cosmica ノート「{title}」の続きを作業してください。\n(元ノート: {path})\n\n---\n\n{body}',
    insertPrompt: 'Cosmica ノート「{title}」({path}):\n\n{body}',
    summaryPrompt: null,
    logTitle: '{title} — 会話ログ',
    user: 'ユーザー',
    assistant: 'アシスタント',
    status: { idle: '待機中', working: '作業中', thinking: '思考中', finished: '完了', permission: '承認待ち', question: '回答待ち', interrupted: '中断', exited: '終了', shell: 'ターミナル' },
  },
  zh: {
    connected: '已连接',
    offline: 'Cosmica 未运行',
    notFound: '未找到 Cosmica',
    notFoundBody: '请安装 Cosmica，或在 ~/CosmicaNotes/Note 中创建笔记。',
    notes: '笔记 → 提示词',
    search: '搜索笔记',
    noNotes: '没有笔记。',
    insert: '插入到当前终端',
    continueIn: '继续工作…',
    session: '会话 → Cosmica',
    noPane: '选择 Claude Code 或 Codex 窗格即可保存其会话。',
    paneStatus: '{agent} · {status}',
    aiSummary: '保存 AI 摘要',
    transcript: '保存对话记录',
    busy: '智能体正在工作，请在完成后重试。',
    needAgent: '请先选择 Claude Code 或 Codex 窗格。',
    asked: '已请智能体将摘要写入 Cosmica…',
    saved: '已保存到 Cosmica：{title}',
    lastSaved: '最近保存',
    reveal: '在访达中显示',
    settings: '设置',
    folder: '保存会话的 Cosmica 文件夹',
    folderSaved: '会话将保存到 {folder}',
    refresh: '刷新',
    summaryTimeout: '摘要文件尚未生成，请查看智能体窗格。',
    continuePrompt: '请继续完成 Cosmica 笔记“{title}”中的工作。\n（原笔记：{path}）\n\n---\n\n{body}',
    insertPrompt: 'Cosmica 笔记“{title}”（{path}）：\n\n{body}',
    summaryPrompt: null,
    logTitle: '{title} — 对话记录',
    user: '用户',
    assistant: '助手',
    status: { idle: '空闲', working: '工作中', thinking: '思考中', finished: '已完成', permission: '等待批准', question: '等待回答', interrupted: '已中断', exited: '已退出', shell: '终端' },
  },
};

const AGENT_NAMES = { claude: 'Claude Code', codex: 'Codex' };

const state = {
  config: null,
  running: false,
  query: '',
  notes: [],
  settings: { folder: 'Agentty' },
  lastSaved: null,
  watching: false,
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
    return;
  }
  state.notes = state.query ? await cosmica.searchNotes(state.config.notesPath, state.query, 40) : await cosmica.recentNotes(state.config.notesPath, 40);
}

function noteLabel(note) {
  return note.folder ? `${note.folder} · ${relativeTime(note.mtimeMs)}` : relativeTime(note.mtimeMs);
}

async function notePrompt(key, notePath) {
  const note = await cosmica.readNote(state.config.notesPath, notePath);
  return { note, text: tr(key, { title: note.title, path: note.path, body: note.body }) };
}

async function continueNote(notePath, context) {
  if (!state.config) state.config = await cosmica.loadConfig();
  const { note, text } = await notePrompt('continuePrompt', notePath);
  await plugin.injectPrompt({ text, title: note.title, target: 'ask', cwd: context?.pane?.cwd });
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
  if (['working', 'thinking', 'permission', 'question'].includes(pane.status)) return plugin.notify(tr('busy'), 'warning');
  if (!state.config) state.config = await cosmica.loadConfig();
  const target = path.join(state.config.notesPath, cosmica.safeFolder(state.settings.folder), cosmica.newNoteName());
  const template = STRINGS[language()].summaryPrompt ?? STRINGS.en.summaryPrompt;
  const text = template.split('{path}').join(target).split('{date}').join(new Date().toISOString()).split('{cwd}').join(pane.cwd);
  await plugin.sendToTerminal({ paneId: pane.id, text, submit: true });
  await plugin.notify(tr('asked'), 'info');
  watchForSummary(target);
}

/** Waits (up to 15 minutes) for the agent to write the summary, then tells Cosmica about it. */
async function watchForSummary(target) {
  const started = Date.now();
  let lastSize = -1;
  while (Date.now() - started < 15 * 60 * 1000) {
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
      await render();
      return;
    }
    lastSize = size;
  }
  await plugin.notify(tr('summaryTimeout'), 'warning');
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
  await render();
}

// -- panel ----------------------------------------------------------------------------------------

async function render() {
  if (!state.panelOpen) return;
  const config = state.config ?? (await cosmica.loadConfig());
  const context = plugin.context;
  const pane = focusedAgent(context);
  const header = ui.row(
    [
      ui.badge(!config.notesExist ? tr('notFound') : state.running ? tr('connected') : tr('offline'), !config.notesExist ? 'error' : state.running ? 'success' : 'neutral'),
      ui.button('refresh', tr('refresh'), { icon: 'refresh-cw', variant: 'ghost' }),
    ],
    { gap: 'small' },
  );
  if (!config.notesExist) {
    return plugin.setPanel(ui.column([header, ui.text(tr('notFoundBody'), 'muted')]));
  }
  const items = state.notes.map((note) => ({
    id: note.path,
    title: note.title,
    subtitle: noteLabel(note),
    icon: note.locked ? 'shield-alert' : note.source === 'agentty' ? 'bot' : 'file-text',
    actions: note.locked
      ? []
      : [
          { id: 'insert', icon: 'file-input', tooltip: tr('insert') },
          { id: 'continue', icon: 'send', tooltip: tr('continueIn') },
        ],
  }));
  await plugin.setPanel(
    ui.column([
      header,
      ui.text(config.notesPath, 'small'),
      ui.section(tr('notes'), [ui.input('query', { placeholder: tr('search'), value: state.query }), ui.list('notes', items, { empty: tr('noNotes') })]),
      ui.section(tr('session'), [
        pane
          ? ui.text(tr('paneStatus', { agent: AGENT_NAMES[pane.kind] ?? pane.kind, status: STRINGS[language()].status[pane.status] ?? pane.status }), 'muted')
          : ui.text(tr('noPane'), 'muted'),
        ui.row(
          [
            ui.button('ai-summary', tr('aiSummary'), { icon: 'sparkles', variant: 'primary', disabled: !pane }),
            ui.button('transcript', tr('transcript'), { icon: 'scroll-text', disabled: !pane }),
          ],
          { wrap: true, gap: 'small' },
        ),
        state.lastSaved && ui.row([ui.text(`${tr('lastSaved')}: ${state.lastSaved.title}`, 'small'), ui.button('reveal', tr('reveal'), { icon: 'folder-open', variant: 'ghost' })], { gap: 'small', wrap: true }),
      ]),
      ui.section(tr('settings'), [ui.text(tr('folder'), 'small'), ui.input('folder', { placeholder: 'Agentty', value: state.settings.folder })]),
    ]),
  );
}

let searchTimer = null;

plugin
  .onActivate(async () => {
    await loadSettings();
    state.config = await cosmica.loadConfig();
  })
  .onPanelOpen(async () => {
    state.panelOpen = true;
    await refresh();
    await render();
  })
  .onPanelClose(() => {
    state.panelOpen = false;
  })
  .onContextChange(() => render())
  .onEvent('refresh', async () => {
    await refresh();
    await render();
  })
  .onEvent('query', async (event) => {
    state.query = String(event.value ?? '');
    clearTimeout(searchTimer);
    searchTimer = setTimeout(async () => {
      await refresh();
      await render();
    }, event.event === 'submit' ? 0 : 200);
  })
  .onEvent('notes', async (event, context) => {
    if (event.event === 'action' && event.action === 'insert') return insertNote(event.item);
    // Clicking a row or its send button continues the note somewhere.
    return continueNote(event.item, context);
  })
  .onEvent('ai-summary', (_event, context) => saveAiSummary(context))
  .onEvent('transcript', (_event, context) => saveTranscript(context))
  .onEvent('reveal', () => state.lastSaved && plugin.revealPath(state.lastSaved.path))
  .onEvent('folder', async (event) => {
    if (event.event !== 'submit' && event.event !== 'change') return;
    state.settings.folder = cosmica.safeFolder(event.value);
    await saveSettings();
    if (event.event === 'submit') await plugin.notify(tr('folderSaved', { folder: state.settings.folder }), 'success');
  })
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
    await continueNote(query.path, plugin.context);
  })
  .start();
