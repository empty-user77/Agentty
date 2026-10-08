// UI Gallery: every element a plugin panel can show, live, with the code that draws it.
// Each example below is the code it shows: the gallery prints the function's own source.
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();

const STRINGS = {
  en: {
    intro: 'Every element a plugin panel can show. Click around: the examples are live.',
    showCode: 'Show the code',
    layout: 'Layout',
    data: 'Data',
    inputs: 'Inputs',
    feedback: 'Feedback',
    recipes: 'Recipes',
    recipesIntro: 'Whole screens to start a plugin from. Copy one, then change the words.',
    clicked: 'Clicked {id}',
  },
  ko: {
    intro: '플러그인 패널에 쓸 수 있는 모든 요소입니다. 직접 눌러 보세요. 예시는 실제로 동작합니다.',
    showCode: '코드 보기',
    layout: '레이아웃',
    data: '데이터',
    inputs: '입력',
    feedback: '피드백',
    recipes: '레시피',
    recipesIntro: '플러그인을 시작할 완성 화면입니다. 하나를 복사한 뒤 문구만 바꾸세요.',
    clicked: '{id} 클릭됨',
  },
  ja: {
    intro: 'プラグインのパネルに置けるすべての要素です。触ってみてください。例は実際に動きます。',
    showCode: 'コードを表示',
    layout: 'レイアウト',
    data: 'データ',
    inputs: '入力',
    feedback: 'フィードバック',
    recipes: 'レシピ',
    recipesIntro: 'プラグインの出発点になる画面一式です。コピーして文言を変えてください。',
    clicked: '{id} をクリックしました',
  },
  zh: {
    intro: '插件面板可以显示的所有元素。动手点点看：示例都是可以操作的。',
    showCode: '显示代码',
    layout: '布局',
    data: '数据',
    inputs: '输入',
    feedback: '反馈',
    recipes: '模板',
    recipesIntro: '可以直接作为插件起点的完整界面。复制一个，再改掉文字即可。',
    clicked: '已点击 {id}',
  },
};

function tr(key, args = {}) {
  const code = plugin.context?.language ?? plugin.info?.language ?? 'en';
  const text = (STRINGS[code] ?? STRINGS.en)[key] ?? STRINGS.en[key] ?? key;
  return text.replace(/\{(\w+)\}/g, (_, name) => args[name] ?? '');
}

// What the user changed in the examples.
const state = {
  tab: 'layout',
  showCode: false,
  recipe: 'listDetail',
  demoTab: 'overview',
  plan: 'pro',
  region: 'seoul',
  notify: true,
  autoSave: false,
  agree: true,
  row: 'build',
  item: 'a',
  progress: 0.42,
  openRequests: ['users', 'create'],
  request: 'create',
};

// ---------------------------------------------------------------- examples

const LAYOUT = [
  {
    title: 'column · row',
    note: 'column stacks, row puts side by side. gap: none | small | medium | large. Text, inputs and blocks in a row share its width.',
    draw: () =>
      ui.column([
        ui.row([ui.text('A row: text takes the room the buttons leave.'), ui.button('row.ok', 'OK', { variant: 'primary' })]),
        ui.row([ui.badge('wrap'), ui.badge('tags'), ui.badge('onto'), ui.badge('lines'), ui.badge('when'), ui.badge('narrow')], { gap: 'small', wrap: true }),
      ]),
  },
  {
    title: 'card',
    note: 'A raised box around what belongs together. tone colors the icon and the edge.',
    draw: () =>
      ui.grid([
        ui.card([ui.text('Two agents are working on it.', 'muted')], { title: 'Build', subtitle: 'main · 2 min ago', icon: 'hammer' }),
        ui.card([ui.text('3 tests failed in auth.', 'muted')], { title: 'Tests', subtitle: 'needs a look', icon: 'bug', tone: 'error' }),
      ]),
  },
  {
    title: 'grid',
    note: 'Equal columns (1–6) that wrap onto new rows. The last row keeps the column widths.',
    draw: () =>
      ui.grid(
        [ui.stat('Sessions', '12'), ui.stat('Tokens', '1.2M'), ui.stat('Cost', '$3.40'), ui.stat('Errors', '0', { tone: 'success' })],
        { columns: 3, gap: 'small' },
      ),
  },
  {
    title: 'tabs',
    note: 'A tab strip. Send only the picked tab\'s content as children; picking one sends change with its id.',
    draw: () =>
      ui.tabs(
        'demo.tabs',
        [
          { id: 'overview', label: 'Overview', icon: 'eye' },
          { id: 'runs', label: 'Runs', badge: '3' },
          { id: 'settings', label: 'Settings', icon: 'settings' },
        ],
        state.demoTab,
        [ui.text(`This is the ${state.demoTab} tab.`, 'muted')],
      ),
  },
  {
    title: 'section · divider',
    note: 'A section names a group with a quiet heading; a divider separates without one.',
    draw: () =>
      ui.column([
        ui.section('Account', [ui.text('signed in as dev@example.com', 'muted')]),
        ui.divider(),
        ui.text('Below the line.', 'muted'),
      ]),
  },
];

const DATA = [
  {
    title: 'text',
    note: 'style: title | body | muted | small | code | success | error',
    draw: () =>
      ui.column([
        ui.text('Title', 'title'),
        ui.text('Body — what most text is.'),
        ui.text('Muted — secondary lines.', 'muted'),
        ui.text('Small — captions and fine print.', 'small'),
        ui.text('npm run build', 'code'),
        ui.row([ui.text('Saved', 'success'), ui.text('Could not connect', 'error')]),
      ], { gap: 'small' }),
  },
  {
    title: 'list',
    note: 'Rows with an icon, title, subtitle and detail. A click sends select; row buttons appear on hover and send action.',
    draw: () =>
      ui.list(
        'demo.list',
        [
          { id: 'a', title: 'Fix the login timeout', subtitle: 'auth · Claude Code', detail: '2 min ago', icon: 'circle-check', tone: 'success', actions: [{ id: 'open', icon: 'external-link', tooltip: 'Open' }] },
          { id: 'b', title: 'Refactor the payment flow', subtitle: 'billing · Codex', detail: 'working…', icon: 'loader-circle', tone: 'info' },
          { id: 'c', title: 'Update dependencies', subtitle: 'chore', icon: 'package', actions: [{ id: 'remove', icon: 'trash-2', tooltip: 'Remove' }] },
        ],
        { empty: 'No tasks yet.' },
      ),
  },
  {
    title: 'table',
    note: 'Rows under headings. grow sets a column\'s share of the width, align puts numbers on the right. A row click sends select.',
    draw: () =>
      ui.table(
        'demo.table',
        [{ label: 'Job', grow: 3 }, { label: 'Branch', grow: 2 }, { label: 'Time', align: 'end' }],
        [
          { id: 'build', cells: ['build', 'main', '1m 12s'], tone: 'success' },
          { id: 'test', cells: ['test', 'main', '3m 40s'], tone: 'error' },
          { id: 'deploy', cells: ['deploy', 'release', '—'] },
        ],
        { selected: state.row },
      ),
  },
  {
    title: 'keyValue',
    note: 'Label and value pairs. mono for ids, hashes and paths; tone colors the value.',
    draw: () =>
      ui.keyValue([
        { label: 'Status', value: 'Running', tone: 'success' },
        { label: 'Commit', value: 'ebfe735', mono: true },
        { label: 'Folder', value: '~/projects/agentty', mono: true },
        { label: 'Started', value: 'Today, 14:02' },
      ]),
  },
  {
    title: 'stat',
    note: 'One number that matters. detail says how it moved; tone colors it.',
    draw: () =>
      ui.grid([
        ui.stat('Passed', '128', { detail: '+4 since yesterday', tone: 'success', icon: 'circle-check' }),
        ui.stat('Failed', '2', { detail: '1 new', tone: 'error', icon: 'circle-x' }),
      ]),
  },
  {
    title: 'badge',
    note: 'A short label. tone: neutral | info | success | warning | error',
    draw: () =>
      ui.row([ui.badge('draft'), ui.badge('info', 'info'), ui.badge('merged', 'success'), ui.badge('stale', 'warning'), ui.badge('failed', 'error')], {
        gap: 'small',
        wrap: true,
      }),
  },
  {
    title: 'code',
    note: 'Monospaced and colored by language (rust, json, ts, sh, …), with a copy button.',
    draw: () => ui.code('{\n  "name": "my-plugin",\n  "apiVersion": 4\n}', { language: 'json', title: 'agentty-plugin.json' }),
  },
];

const INPUTS = [
  {
    title: 'button',
    note: 'variant: primary (one per screen) | secondary | ghost | danger. An icon goes before the label.',
    draw: () =>
      ui.row([
        ui.button('btn.primary', 'Deploy', { variant: 'primary', icon: 'rocket' }),
        ui.button('btn.secondary', 'Preview'),
        ui.button('btn.ghost', 'Cancel', { variant: 'ghost' }),
        ui.button('btn.danger', 'Delete', { variant: 'danger', icon: 'trash-2' }),
      ], { gap: 'small', wrap: true }),
  },
  {
    title: 'input',
    note: 'change while typing (after a pause), submit on Enter. rows > 1 makes a text area.',
    draw: () =>
      ui.column([
        ui.input('demo.name', { placeholder: 'Project name' }),
        ui.input('demo.notes', { placeholder: 'Notes for the agent…', rows: 3 }),
      ], { gap: 'small' }),
  },
  {
    title: 'select',
    note: 'A drop-down for more than a handful of options. Picking one sends change with its value.',
    draw: () =>
      ui.select(
        'demo.region',
        [
          { value: 'seoul', label: 'Seoul (ap-northeast-2)' },
          { value: 'tokyo', label: 'Tokyo (ap-northeast-1)' },
          { value: 'virginia', label: 'N. Virginia (us-east-1)' },
          { value: 'frankfurt', label: 'Frankfurt (eu-central-1)' },
        ],
        state.region,
        { placeholder: 'Pick a region' },
      ),
  },
  {
    title: 'choice',
    note: 'A segmented choice for two to four options that should all stay visible.',
    draw: () =>
      ui.choice('demo.plan', [{ value: 'free', label: 'Free' }, { value: 'pro', label: 'Pro' }, { value: 'team', label: 'Team' }], state.plan),
  },
  {
    title: 'checkbox · toggle',
    note: 'A checkbox for something to agree to or pick; a toggle for a setting that takes effect at once.',
    draw: () =>
      ui.column([
        ui.checkbox('demo.agree', 'Open a pull request when done', state.agree, { description: 'The agent pushes its branch and opens a draft PR.' }),
        ui.toggle('demo.notify', 'Notify me when an agent finishes', state.notify),
        ui.toggle('demo.autosave', 'Save drafts automatically', state.autoSave),
      ]),
  },
];

const FEEDBACK = [
  {
    title: 'callout',
    note: 'A tinted note. tone: info (default) | success | warning | error | neutral',
    draw: () =>
      ui.column([
        ui.callout('Agents can read this folder. Nothing leaves your computer.', { title: 'Local only' }),
        ui.callout('Your token expires in 3 days.', { tone: 'warning' }),
        ui.callout('Could not reach api.example.com (timed out after 10 s).', { title: 'Sync failed', tone: 'error' }),
      ], { gap: 'small' }),
  },
  {
    title: 'progress',
    note: 'A bar filled from 0 to 1. detail replaces the percentage.',
    draw: () =>
      ui.column([
        ui.progress(state.progress, { label: 'Uploading build' }),
        ui.progress(1, { label: 'Tests', detail: '128 / 128', tone: 'success' }),
        ui.row([ui.button('demo.less', '−10%', { variant: 'ghost' }), ui.button('demo.more', '+10%', { variant: 'ghost' })], { gap: 'small' }),
      ]),
  },
  {
    title: 'spinner',
    note: 'Something is loading or running.',
    draw: () => ui.spinner('Waiting for the agent…'),
  },
  {
    title: 'flow',
    note: 'Steps of an automation, top to bottom. state: off | on | active | done | error. A click sends select.',
    draw: () =>
      ui.flow('demo.flow', [
        { id: 'collect', title: 'Collect issues', subtitle: '12 found', icon: 'search', state: 'done' },
        { id: 'triage', title: 'Triage with Claude Code', subtitle: 'working…', icon: 'bot', state: 'active', selected: true },
        { id: 'post', title: 'Post a summary', icon: 'send', state: 'on' },
      ]),
  },
];

// ---------------------------------------------------------------- recipes

const RECIPES = [
  {
    id: 'apiClient',
    label: 'API client',
    draw: () =>
      ui.grid(
        [
          ui.list('recipe.tree', [
            { id: 'fleet', title: 'Fleet API', icon: 'package', tone: 'info' },
            { id: 'vehicles', title: 'vehicles', icon: 'folder-open', depth: 1 },
            { id: 'users', title: 'List vehicles', tag: 'GET', tagTone: 'success', depth: 2 },
            { id: 'create', title: 'Create vehicle', tag: 'POST', tagTone: 'warning', depth: 2 },
            { id: 'remove', title: 'Delete vehicle', tag: 'DEL', tagTone: 'error', depth: 2 },
          ]),
          ui.tabs(
            'recipe.requests',
            state.openRequests.map((id) => ({ id, label: id === 'users' ? 'GET List vehicles' : 'POST Create vehicle', closable: true })),
            state.request,
            [
              ui.row([ui.select('recipe.method', [{ value: 'GET', label: 'GET' }, { value: 'POST', label: 'POST' }], state.request === 'users' ? 'GET' : 'POST'), ui.input('recipe.url2', { value: '{{baseUrl}}/vehicles' }), ui.button('recipe.send', 'Send', { variant: 'primary', icon: 'send' })]),
              ui.input('recipe.body', { value: '{\n  "make": "Volvo",\n  "year": 2024\n}', rows: 5, mono: true }),
            ],
          ),
        ],
        { widths: ['200px', '1'], gap: 'small' },
      ),
  },
  {
    id: 'listDetail',
    label: 'List + detail',
    draw: () =>
      ui.column([
        ui.row([ui.input('recipe.search', { placeholder: 'Search tasks' }), ui.button('recipe.new', 'New', { variant: 'primary', icon: 'plus' })]),
        ui.list('recipe.items', [
          { id: 'a', title: 'Fix the login timeout', subtitle: 'Claude Code · 2 min ago', icon: 'circle-check', tone: 'success' },
          { id: 'b', title: 'Refactor the payment flow', subtitle: 'Codex · working', icon: 'loader-circle', tone: 'info' },
        ]),
        state.item &&
          ui.card(
            [
              ui.keyValue([
                { label: 'Status', value: state.item === 'a' ? 'Done' : 'Working', tone: state.item === 'a' ? 'success' : 'info' },
                { label: 'Branch', value: state.item === 'a' ? 'fix/login-timeout' : 'refactor/payments', mono: true },
              ]),
              ui.row([ui.button('recipe.open', 'Open in a tab', { icon: 'external-link' }), ui.button('recipe.delete', 'Delete', { variant: 'ghost' })], { gap: 'small' }),
            ],
            { title: state.item === 'a' ? 'Fix the login timeout' : 'Refactor the payment flow', icon: 'file-text' },
          ),
      ]),
  },
  {
    id: 'settings',
    label: 'Settings form',
    draw: () =>
      ui.column([
        ui.section('Connection', [
          ui.input('recipe.url', { placeholder: 'https://api.example.com' }),
          ui.select('recipe.env', [{ value: 'dev', label: 'Development' }, { value: 'prod', label: 'Production' }], 'dev'),
        ]),
        ui.section('Behavior', [
          ui.toggle('recipe.watch', 'Watch for changes', true),
          ui.checkbox('recipe.confirm', 'Ask before sending prompts', true, { description: 'Shows the prompt to the user first.' }),
        ]),
        ui.row([ui.button('recipe.save', 'Save', { variant: 'primary' }), ui.button('recipe.reset', 'Reset', { variant: 'ghost' })], { gap: 'small' }),
      ]),
  },
  {
    id: 'dashboard',
    label: 'Dashboard',
    draw: () =>
      ui.column([
        ui.grid([
          ui.stat('Open PRs', '7', { detail: '+2 today', tone: 'info', icon: 'git-pull-request' }),
          ui.stat('CI', 'passing', { detail: 'main · 4 min ago', tone: 'success', icon: 'circle-check' }),
        ]),
        ui.section('Recent runs', [
          ui.table(
            'recipe.runs',
            [{ label: 'Workflow', grow: 3 }, { label: 'Result', grow: 2 }, { label: 'Time', align: 'end' }],
            [
              { id: '1', cells: ['ci.yml', 'success', '4m'], tone: 'success' },
              { id: '2', cells: ['release.yml', 'failed', '12m'], tone: 'error' },
            ],
          ),
        ]),
      ]),
  },
  {
    id: 'job',
    label: 'Running a job',
    draw: () =>
      ui.column([
        ui.card(
          [
            ui.progress(0.6, { label: 'Step 3 of 5 · Running tests', detail: '60%' }),
            ui.code('$ cargo test --workspace\n   Compiling agentty-app\n    Finished test profile', { language: 'sh' }),
            ui.row([ui.button('recipe.stop', 'Stop', { variant: 'danger', icon: 'square' })]),
          ],
          { title: 'Release check', subtitle: 'started 2 min ago', icon: 'rocket', tone: 'info' },
        ),
      ]),
  },
  {
    id: 'states',
    label: 'Empty & error',
    draw: () =>
      ui.column([
        ui.list('recipe.empty', [], { empty: 'No notes yet. Create one to get started.' }),
        ui.callout('Check the token in Settings and try again.', { title: 'Could not sign in', tone: 'error' }),
        ui.row([ui.button('recipe.retry', 'Try again', { variant: 'primary', icon: 'refresh-cw' })]),
      ]),
  },
];

// ---------------------------------------------------------------- drawing

/** The body of an example's function, as it reads in this file. */
function source(fn) {
  const text = fn.toString().replace(/^\(\)\s*=>\s*/, '');
  const lines = text.split('\n');
  const indents = lines.slice(1).filter((l) => l.trim()).map((l) => l.match(/^ */)[0].length);
  const cut = indents.length ? Math.min(...indents) : 0;
  return [lines[0], ...lines.slice(1).map((l) => l.slice(cut))].join('\n').trim();
}

function example({ title, note, draw }) {
  return ui.card([draw(), state.showCode && ui.code(source(draw), { language: 'js' })], { title, subtitle: note });
}

function render() {
  const sections = { layout: LAYOUT, data: DATA, inputs: INPUTS, feedback: FEEDBACK };
  const tabs = [
    { id: 'layout', label: tr('layout') },
    { id: 'data', label: tr('data') },
    { id: 'inputs', label: tr('inputs') },
    { id: 'feedback', label: tr('feedback') },
    { id: 'recipes', label: tr('recipes'), icon: 'sparkles' },
  ];
  let body;
  if (state.tab === 'recipes') {
    const recipe = RECIPES.find((r) => r.id === state.recipe) ?? RECIPES[0];
    body = [
      ui.text(tr('recipesIntro'), 'muted'),
      ui.select('gallery.recipe', RECIPES.map((r) => ({ value: r.id, label: r.label })), recipe.id),
      recipe.draw(),
      ui.code(source(recipe.draw), { language: 'js', title: `${recipe.label} — main.mjs` }),
    ];
  } else {
    body = sections[state.tab].map(example);
  }
  return plugin.setPanel(
    ui.column([
      ui.text(tr('intro'), 'muted'),
      ui.toggle('gallery.code', tr('showCode'), state.showCode),
      ui.tabs('gallery.tabs', tabs, state.tab, body),
    ]),
  );
}

const set = (key) => (event) => {
  state[key] = event.value;
  return render();
};

plugin
  .onPanelOpen(render)
  .onEvent('gallery.tabs', set('tab'))
  .onEvent('gallery.code', set('showCode'))
  .onEvent('gallery.recipe', set('recipe'))
  .onEvent('demo.tabs', set('demoTab'))
  .onEvent('demo.plan', set('plan'))
  .onEvent('demo.region', set('region'))
  .onEvent('demo.notify', set('notify'))
  .onEvent('demo.autosave', set('autoSave'))
  .onEvent('demo.agree', set('agree'))
  .onEvent('demo.table', (event) => {
    state.row = event.item;
    return render();
  })
  .onEvent('recipe.tree', (event) => {
    if (['users', 'create'].includes(event.item)) {
      if (!state.openRequests.includes(event.item)) state.openRequests.push(event.item);
      state.request = event.item;
    }
    return render();
  })
  .onEvent('recipe.requests', (event) => {
    if (event.event === 'close') {
      state.openRequests = state.openRequests.filter((id) => id !== event.value);
      if (state.request === event.value) state.request = state.openRequests[0] ?? '';
    } else {
      state.request = event.value;
    }
    return render();
  })
  .onEvent('recipe.items', (event) => {
    state.item = event.item;
    return render();
  })
  .onEvent('demo.less', () => {
    state.progress = Math.max(0, Math.round((state.progress - 0.1) * 10) / 10);
    return render();
  })
  .onEvent('demo.more', () => {
    state.progress = Math.min(1, Math.round((state.progress + 0.1) * 10) / 10);
    return render();
  })
  .onAnyEvent((event) => {
    if (event.event === 'click' || event.event === 'action') {
      return plugin.notify(tr('clicked', { id: event.action ? `${event.element} → ${event.action}` : event.element }), 'info');
    }
  })
  .start();

