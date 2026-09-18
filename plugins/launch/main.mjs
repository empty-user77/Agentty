// Launch for Agentty: takes a project from a folder on disk to a live URL — GitHub login, save the
// project to a GitHub repo, Vercel login, environment variables, production deploy, done. Every step
// shows one primary button; nothing runs until the user presses it.

import path from 'node:path';
import { existsSync, statSync } from 'node:fs';
import { createPlugin, ui } from './agentty-plugin.mjs';
import { run, tailLines } from './lib/exec.mjs';
import { ensureGh, ensureVercel, findGh, findVercel } from './lib/tools.mjs';
import {
  findProjectRoot,
  inspectProject,
  ghAuthStatus,
  ghUserIdentity,
  ghLogin,
  ensureGitRepo,
  ensureGitignore,
  envFilesToRefuse,
  setLocalGitUserIfMissing,
  commitAll,
  hasOrigin,
  pushOrigin,
  createAndPushRepo,
  repoUrl,
} from './lib/github.mjs';
import { vercelWhoami, vercelLogin, discoverEnvVars, addEnvVar, deployProduction, connectGit } from './lib/vercel.mjs';
import { loadProject, saveProject } from './lib/state.mjs';

const plugin = createPlugin();

// -- copy -------------------------------------------------------------------------------------

const STRINGS = {
  en: {
    title: 'Launch',
    refresh: 'Refresh',
    noPane: 'Focus a terminal in your project folder, then reopen Launch.',
    noProjectTitle: "This doesn't look like a web project yet",
    noProjectBody: 'No package.json or index.html was found in {root}. Ask your agent to build something here first, then reopen Launch.',
    stepProject: 'Project',
    stepTools: 'Tools',
    stepGithubLogin: 'GitHub login',
    stepGithubSave: 'Save to GitHub',
    stepVercelLogin: 'Vercel login',
    stepEnv: 'Environment variables',
    stepDeploy: 'Publish',
    frameworkLine: '{framework} project',
    frameworkUnknown: 'Static site',
    toolsBody: 'Launch needs two small command-line tools: the GitHub CLI and the Vercel CLI. They install straight into Launch, nothing system-wide.',
    installTools: 'Install tools',
    installingTools: 'Installing tools…',
    ghLoginBody: "You'll sign in to GitHub in your browser — nothing to type here.",
    ghLoginButton: 'Log in to GitHub',
    ghLoginCodeTitle: 'Enter this code on GitHub',
    ghLoginWaiting: 'Waiting for you to finish in the browser…',
    ghLoggingIn: 'Opening GitHub…',
    ghLoggedInAs: 'Signed in to GitHub as {username}',
    copyCode: 'Copy code',
    codeCopied: 'Code copied',
    openGithub: 'Open GitHub',
    openVercel: 'Open Vercel',
    githubSaveBody: 'This saves your project as a private GitHub repository (you can make it public below).',
    publicRepo: 'Make the repository public',
    githubSaveButton: 'Save to GitHub',
    savingToGithub: 'Saving to GitHub…',
    envGuardError: 'Refusing to save: {files} would be uploaded with real values in it. Move it out of the project or add it to .gitignore, then try again.',
    repoSaved: 'Saved to {url}',
    vercelLoginBody: "Now you'll sign in to Vercel, the free service that puts your site on the internet.",
    vercelLoginButton: 'Log in to Vercel',
    vercelLoggingIn: 'Opening Vercel…',
    vercelLoginCodeTitle: 'Enter this code on Vercel',
    vercelLoggedInAs: 'Signed in to Vercel as {username}',
    vercelFallbackBody: "Vercel needs a browser step Launch can't drive by itself. Log in from a terminal, then come back here.",
    vercelLoginTerminal: 'Log in in a terminal',
    vercelLoginTerminalHint: 'A terminal opened with the login command — press Enter there, finish in your browser, then come back and press "Check again".',
    checkAgain: 'Check again',
    stillNotLoggedIn: "Vercel isn't signed in yet.",
    envBody: 'These were found in your project. Add them to the live site so it works the same as on your computer. Only the names are shown here — never the values.',
    envLocal: 'points at your computer — probably not needed live',
    envAdd: 'Add to Vercel',
    envSkip: 'Skip for now',
    addingEnv: 'Adding environment variables…',
    deployBody: 'Everything is ready. This puts your project live on the internet.',
    deployButton: 'Publish',
    deploying: 'Publishing…',
    launchedTitle: 'Launched!',
    launchedBody: 'Your site is live at:',
    openSite: 'Open site',
    copyLink: 'Copy link',
    linkCopied: 'Link copied',
    updateSite: 'Update site',
    updatingSite: 'Updating…',
    openRepo: 'Open GitHub repo',
    lastPublished: 'Last published {time}',
    autoDeployHint: "Couldn't connect automatic deploys — you can turn that on in the Vercel project settings.",
    tryAgain: 'Try again',
    askAgent: 'Ask the agent to fix it',
    askedAgent: 'Asked the agent to look into it…',
    fixPrompt:
      'Publishing this project with Agentty Launch failed at this step: {step}.\n\nCommand: {command}\n\nOutput (last lines):\n{log}\n\nPlease fix the project so it builds and deploys successfully on Vercel. Do not commit any secrets or .env files with real values.',
  },
  ko: {
    title: '출시',
    refresh: '새로고침',
    noPane: '프로젝트 폴더의 터미널을 선택한 뒤 Launch를 다시 열어 주세요.',
    noProjectTitle: '아직 웹 프로젝트가 아닌 것 같아요',
    noProjectBody: '{root} 에서 package.json이나 index.html을 찾지 못했습니다. 에이전트에게 먼저 이 폴더에 앱을 만들어 달라고 한 뒤 다시 열어 주세요.',
    stepProject: '프로젝트',
    stepTools: '도구',
    stepGithubLogin: 'GitHub 로그인',
    stepGithubSave: 'GitHub에 저장',
    stepVercelLogin: 'Vercel 로그인',
    stepEnv: '환경 변수',
    stepDeploy: '출시',
    frameworkLine: '{framework} 프로젝트',
    frameworkUnknown: '정적 사이트',
    toolsBody: 'Launch는 작은 명령줄 도구 두 개가 필요합니다: GitHub CLI와 Vercel CLI. Launch 안에만 설치되며, 시스템 전체에는 영향을 주지 않습니다.',
    installTools: '도구 설치',
    installingTools: '도구 설치 중…',
    ghLoginBody: '브라우저에서 GitHub에 로그인합니다 — 여기에 입력할 건 없어요.',
    ghLoginButton: 'GitHub에 로그인',
    ghLoginCodeTitle: 'GitHub에 이 코드를 입력하세요',
    ghLoginWaiting: '브라우저에서 로그인을 마치면 자동으로 진행됩니다…',
    ghLoggingIn: 'GitHub 여는 중…',
    ghLoggedInAs: 'GitHub에 {username}(으)로 로그인됨',
    copyCode: '코드 복사',
    codeCopied: '코드를 복사했습니다',
    openGithub: 'GitHub 열기',
    openVercel: 'Vercel 열기',
    githubSaveBody: '프로젝트를 비공개 GitHub 저장소로 저장합니다 (아래에서 공개로 바꿀 수 있어요).',
    publicRepo: '저장소를 공개로 만들기',
    githubSaveButton: 'GitHub에 저장',
    savingToGithub: 'GitHub에 저장 중…',
    envGuardError: '저장을 중단했습니다: {files} 에 실제 값이 들어 있어 업로드될 뻔했습니다. 프로젝트 밖으로 옮기거나 .gitignore에 추가한 뒤 다시 시도해 주세요.',
    repoSaved: '{url} 에 저장됨',
    vercelLoginBody: '이제 사이트를 인터넷에 올려주는 무료 서비스인 Vercel에 로그인합니다.',
    vercelLoginButton: 'Vercel에 로그인',
    vercelLoggingIn: 'Vercel 여는 중…',
    vercelLoginCodeTitle: 'Vercel에 이 코드를 입력하세요',
    vercelLoggedInAs: 'Vercel에 {username}(으)로 로그인됨',
    vercelFallbackBody: 'Vercel 로그인은 Launch가 대신할 수 없는 브라우저 단계가 필요합니다. 터미널에서 로그인한 뒤 돌아와 주세요.',
    vercelLoginTerminal: '터미널에서 로그인',
    vercelLoginTerminalHint: '로그인 명령이 입력된 터미널이 열렸습니다 — 거기서 Enter를 누르고 브라우저에서 로그인을 마친 뒤, 돌아와서 "다시 확인"을 눌러 주세요.',
    checkAgain: '다시 확인',
    stillNotLoggedIn: '아직 Vercel에 로그인되지 않았습니다.',
    envBody: '프로젝트에서 이 값들을 찾았습니다. 실제 사이트에서도 컴퓨터에서와 똑같이 동작하도록 추가하세요. 이름만 보여지고 값은 표시되지 않습니다.',
    envLocal: '내 컴퓨터를 가리킴 — 실제 사이트에는 필요 없을 수 있어요',
    envAdd: 'Vercel에 추가',
    envSkip: '나중에 하기',
    addingEnv: '환경 변수 추가 중…',
    deployBody: '모든 준비가 끝났습니다. 이제 프로젝트를 인터넷에 공개합니다.',
    deployButton: '인터넷에 공개하기',
    deploying: '공개하는 중…',
    launchedTitle: '출시 완료!',
    launchedBody: '사이트가 아래 주소에서 공개되었습니다:',
    openSite: '사이트 열기',
    copyLink: '링크 복사',
    linkCopied: '링크를 복사했습니다',
    updateSite: '사이트 업데이트',
    updatingSite: '업데이트 중…',
    openRepo: 'GitHub 저장소 열기',
    lastPublished: '마지막 출시: {time}',
    autoDeployHint: '자동 배포 연결에 실패했습니다 — Vercel 프로젝트 설정에서 직접 켤 수 있어요.',
    tryAgain: '다시 시도',
    askAgent: '에이전트에게 고쳐달라고 하기',
    askedAgent: '에이전트에게 확인을 요청했습니다…',
    fixPrompt:
      'Agentty Launch로 이 프로젝트를 출시하는 중 다음 단계에서 실패했습니다: {step}.\n\n명령어: {command}\n\n출력 (마지막 부분):\n{log}\n\nVercel에서 빌드와 배포가 성공하도록 프로젝트를 고쳐 주세요. 비밀 값이나 실제 값이 담긴 .env 파일은 커밋하지 마세요.',
  },
  ja: {
    title: 'ローンチ',
    refresh: '更新',
    stepProject: 'プロジェクト',
    stepTools: 'ツール',
    stepGithubLogin: 'GitHub ログイン',
    stepGithubSave: 'GitHub に保存',
    stepVercelLogin: 'Vercel ログイン',
    stepEnv: '環境変数',
    stepDeploy: '公開',
    installTools: 'ツールをインストール',
    ghLoginButton: 'GitHub にログイン',
    copyCode: 'コードをコピー',
    codeCopied: 'コピーしました',
    openGithub: 'GitHub を開く',
    publicRepo: 'リポジトリを公開にする',
    githubSaveButton: 'GitHub に保存',
    vercelLoginButton: 'Vercel にログイン',
    vercelLoginTerminal: 'ターミナルでログイン',
    checkAgain: 'もう一度確認',
    envAdd: 'Vercel に追加',
    envSkip: '後で',
    deployButton: '公開する',
    launchedTitle: '公開しました！',
    openSite: 'サイトを開く',
    copyLink: 'リンクをコピー',
    linkCopied: 'コピーしました',
    updateSite: 'サイトを更新',
    openRepo: 'GitHub リポジトリを開く',
    tryAgain: 'もう一度試す',
    askAgent: 'エージェントに修正を依頼',
    askedAgent: 'エージェントに依頼しました…',
  },
  zh: {
    title: '发布',
    refresh: '刷新',
    stepProject: '项目',
    stepTools: '工具',
    stepGithubLogin: 'GitHub 登录',
    stepGithubSave: '保存到 GitHub',
    stepVercelLogin: 'Vercel 登录',
    stepEnv: '环境变量',
    stepDeploy: '发布',
    installTools: '安装工具',
    ghLoginButton: '登录 GitHub',
    copyCode: '复制代码',
    codeCopied: '已复制',
    openGithub: '打开 GitHub',
    publicRepo: '设为公开仓库',
    githubSaveButton: '保存到 GitHub',
    vercelLoginButton: '登录 Vercel',
    vercelLoginTerminal: '在终端登录',
    checkAgain: '再次检查',
    envAdd: '添加到 Vercel',
    envSkip: '暂时跳过',
    deployButton: '发布到互联网',
    launchedTitle: '已发布！',
    openSite: '打开网站',
    copyLink: '复制链接',
    linkCopied: '已复制',
    updateSite: '更新网站',
    openRepo: '打开 GitHub 仓库',
    tryAgain: '重试',
    askAgent: '请智能体修复',
    askedAgent: '已请智能体查看…',
  },
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

// -- state --------------------------------------------------------------------------------------

const STEP_ORDER = ['tools', 'gh-login', 'gh-save', 'vercel-login', 'env', 'deploy'];

const state = {
  panelOpen: false,
  root: null,
  forcedRoot: null,
  inspect: null,
  step: null, // null while loading, 'no-project', one of STEP_ORDER, or 'launched'
  running: false,
  busyLabel: '',
  progressLines: [],
  ghBin: null,
  vercelBin: null,
  gh: { loggedIn: false, username: null },
  vercel: { loggedIn: false, username: null },
  loginCode: null, // { tool: 'github' | 'vercel', code, url }
  vercelFallback: false,
  publicRepo: false,
  envDiscovery: null,
  envSelection: {},
  deployLog: [],
  launched: null, // { url, time }
  saved: null,
  gitConnected: null,
  error: null, // { step, message, command }
  lastAction: null, // { step, label, command, run }
};

function dataDir() {
  return plugin.info?.plugin.dataDir ?? '.';
}

// -- panel dispatch -------------------------------------------------------------------------------

async function runStep(step, label, command, fn) {
  state.running = true;
  state.busyLabel = label;
  state.progressLines = [];
  state.error = null;
  state.lastAction = { step, label, command, run: () => runStep(step, label, command, fn) };
  await render();
  try {
    await fn();
    state.error = null;
  } catch (err) {
    state.error = { step, message: err?.message ?? String(err), command };
  } finally {
    state.running = false;
    state.busyLabel = '';
    state.loginCode = null;
    await render();
  }
}

function progress(line) {
  state.progressLines = [...state.progressLines, line].slice(-8);
  render();
}

let lastDeployRender = 0;
let deployRenderPending = false;
function renderDeployLogThrottled() {
  const now = Date.now();
  if (now - lastDeployRender > 400) {
    lastDeployRender = now;
    render();
  } else if (!deployRenderPending) {
    deployRenderPending = true;
    setTimeout(() => {
      deployRenderPending = false;
      lastDeployRender = Date.now();
      render();
    }, 400);
  }
}

async function copyToClipboard(text) {
  try {
    await run('pbcopy', [], { input: text, timeoutMs: 5000 });
  } catch {
    // Best effort — the code/link is still shown on screen.
  }
}

// -- project detection ------------------------------------------------------------------------

async function openProject(cwd) {
  state.error = null;
  state.step = null;
  await render();
  if (!cwd) {
    state.root = null;
    state.step = 'no-pane';
    return render();
  }
  const root = await findProjectRoot(cwd);
  state.root = root;
  state.inspect = await inspectProject(root);
  if (!state.inspect.hasPackageJson && !state.inspect.hasIndexHtml) {
    state.step = 'no-project';
    return render();
  }
  state.saved = await loadProject(dataDir(), root);
  state.publicRepo = false;
  state.launched = state.saved?.lastDeployUrl ? { url: state.saved.lastDeployUrl, time: state.saved.lastDeployTime } : null;
  await resume();
}

/** Re-checks from wherever things stand and advances `state.step` to the next actionable one. */
async function resume() {
  state.ghBin = await findGh(dataDir());
  state.vercelBin = await findVercel(dataDir());
  if (!state.ghBin || !state.vercelBin) {
    state.step = 'tools';
    return render();
  }
  state.gh = await ghAuthStatus(state.ghBin, state.root);
  if (!state.gh.loggedIn) {
    state.step = 'gh-login';
    return render();
  }
  const origin = await hasOrigin(state.root);
  if (!origin) {
    state.step = 'gh-save';
    return render();
  }
  state.repoUrl = await repoUrl(state.ghBin, state.root);
  state.vercel = await vercelWhoami(state.vercelBin, state.root);
  if (!state.vercel.loggedIn) {
    state.step = 'vercel-login';
    return render();
  }
  state.envDiscovery = await discoverEnvVars(state.root);
  if (state.envDiscovery.files.length > 0 && !state.saved?.envVarsDone) {
    state.envSelection = Object.fromEntries(Object.keys(state.envDiscovery.vars).map((k) => [k, true]));
    state.step = 'env';
    return render();
  }
  if (state.launched) {
    state.step = 'launched';
    return render();
  }
  state.step = 'deploy';
  return render();
}

// -- step actions ---------------------------------------------------------------------------------

function installTools() {
  return runStep('tools', tr('installingTools'), 'gh / vercel install', async () => {
    state.ghBin = await ensureGh(dataDir(), { log: progress });
    state.vercelBin = await ensureVercel(dataDir(), { log: progress });
    await resume();
  });
}

function startGithubLogin() {
  return runStep('gh-login', tr('ghLoggingIn'), 'gh auth login --web', async () => {
    const result = await ghLogin(state.ghBin, state.root, {
      onCode: async (code, url) => {
        state.loginCode = { tool: 'github', code, url };
        await render();
        await copyToClipboard(code);
        try {
          await plugin.openUrl(url);
        } catch {
          // The user can still open it themselves from the code shown.
        }
      },
    });
    if (!result.ok) throw new Error('GitHub login did not finish.');
    state.gh = { loggedIn: true, username: result.username };
    await resume();
  });
}

async function doGithubSave() {
  await ensureGitRepo(state.root);
  await ensureGitignore(state.root);
  const risky = await envFilesToRefuse(state.root);
  if (risky.length > 0) throw new Error(tr('envGuardError', { files: risky.join(', ') }));
  const identity = state.gh.username ? await ghUserIdentity(state.ghBin, state.root) : null;
  if (identity) {
    await setLocalGitUserIfMissing(state.root, { name: identity.login, email: `${identity.id}+${identity.login}@users.noreply.github.com` });
  }
  const origin = await hasOrigin(state.root);
  const message = origin ? `Update via Agentty Launch — ${new Date().toISOString()}` : 'Launch with Agentty';
  await commitAll(state.root, message);
  if (!origin) {
    const folderName = path.basename(state.root);
    await createAndPushRepo(state.ghBin, state.root, { name: folderName, isPublic: state.publicRepo });
  } else {
    await pushOrigin(state.root);
  }
  state.repoUrl = await repoUrl(state.ghBin, state.root);
  await saveProject(dataDir(), state.root, { repoUrl: state.repoUrl });
}

function startGithubSave() {
  return runStep('gh-save', tr('savingToGithub'), 'git commit / gh repo create / git push', async () => {
    await doGithubSave();
    await resume();
  });
}

function startVercelLogin() {
  return runStep('vercel-login', tr('vercelLoggingIn'), 'vercel login', async () => {
    const result = await vercelLogin(state.vercelBin, state.root, {
      onCode: async (code, url) => {
        state.loginCode = { tool: 'vercel', code, url };
        await render();
        await copyToClipboard(code);
        try {
          await plugin.openUrl(url);
        } catch {
          // The user can still open it themselves from the code shown.
        }
      },
    });
    if (result.needsFallback) {
      state.vercelFallback = true;
      return;
    }
    if (!result.ok) throw new Error('Vercel login did not finish.');
    state.vercel = { loggedIn: true, username: result.username };
    state.vercelFallback = false;
    await resume();
  });
}

async function openVercelLoginTerminal() {
  await plugin.injectPrompt({ target: 'newTab', agent: 'shell', text: `${state.vercelBin} login`, cwd: state.root });
  await plugin.notify(tr('vercelLoginTerminalHint'), 'info');
}

function checkVercelLoginAgain() {
  return runStep('vercel-login', tr('checkAgain'), 'vercel whoami', async () => {
    state.vercel = await vercelWhoami(state.vercelBin, state.root);
    if (!state.vercel.loggedIn) throw new Error(tr('stillNotLoggedIn'));
    state.vercelFallback = false;
    await resume();
  });
}

function startEnvAdd() {
  return runStep('env', tr('addingEnv'), 'vercel env add', async () => {
    for (const [key, selected] of Object.entries(state.envSelection)) {
      if (!selected) continue;
      const entry = state.envDiscovery.vars[key];
      if (!entry) continue;
      await addEnvVar(state.vercelBin, state.root, key, entry.value);
    }
    await saveProject(dataDir(), state.root, { envVarsDone: true });
    await resume();
  });
}

function skipEnv() {
  return runStep('env', '', '', async () => {
    await saveProject(dataDir(), state.root, { envVarsDone: true });
    await resume();
  });
}

async function doDeploy() {
  state.deployLog = [];
  const result = await deployProduction(state.vercelBin, state.root, {
    onLine: (lines) => {
      state.deployLog = lines;
      renderDeployLogThrottled();
    },
  });
  await saveProject(dataDir(), state.root, { lastDeployUrl: result.url, lastDeployTime: Date.now() });
  state.gitConnected = await connectGit(state.vercelBin, state.root);
  state.launched = { url: result.url, time: Date.now() };
}

function startDeploy() {
  return runStep('deploy', tr('deploying'), 'vercel deploy --prod --yes', async () => {
    await doDeploy();
    state.step = 'launched';
    await plugin.notify(tr('launchedTitle'), 'success');
    await render();
  });
}

function startUpdateSite() {
  return runStep('deploy', tr('updatingSite'), 'git push / vercel deploy --prod --yes', async () => {
    await doGithubSave();
    await doDeploy();
    state.step = 'launched';
    await plugin.notify(tr('launchedTitle'), 'success');
    await render();
  });
}

async function askAgentToFix() {
  if (!state.error) return;
  await plugin.injectPrompt({
    target: 'ask',
    cwd: state.root,
    title: tr('title'),
    text: tr('fixPrompt', { step: state.error.step, command: state.error.command || '(n/a)', log: tailLines(state.error.message, 60).join('\n') }),
  });
  await plugin.notify(tr('askedAgent'), 'info');
}

// -- rendering --------------------------------------------------------------------------------

const STEP_ICON = { waiting: 'circle-pause', running: 'loader-circle', done: 'circle-check', failed: 'circle-x' };

function statusOf(id) {
  if (state.step === 'no-project' || state.step === 'no-pane' || state.step === null) return 'waiting';
  const order = STEP_ORDER.indexOf(id);
  // "Update site" reruns the save+deploy steps while state.step stays 'launched'; track the step
  // actually running (or that just failed) via lastAction so the checklist reflects it.
  const active = state.running ? state.lastAction?.step : state.error ? state.error.step : state.step;
  const current = active === 'launched' || active === undefined ? STEP_ORDER.length : STEP_ORDER.indexOf(active);
  if (state.error?.step === id) return 'failed';
  if (order < current) return 'done';
  if (order === current) return state.running ? 'running' : 'waiting';
  return 'waiting';
}

function stepsChecklist() {
  const rows = [
    { id: 'tools', label: tr('stepTools') },
    { id: 'gh-login', label: tr('stepGithubLogin') },
    { id: 'gh-save', label: tr('stepGithubSave') },
    { id: 'vercel-login', label: tr('stepVercelLogin') },
    { id: 'env', label: tr('stepEnv') },
    { id: 'deploy', label: tr('stepDeploy') },
  ];
  const items = rows.map((r) => ({ id: r.id, title: r.label, icon: STEP_ICON[statusOf(r.id)] }));
  return ui.list('steps', items);
}

function errorBlock() {
  if (!state.error) return null;
  // 'update-site' reruns the 'deploy' step while state.step stays 'launched'.
  const matches = state.error.step === state.step || (state.step === 'launched' && state.error.step === 'deploy');
  if (!matches) return null;
  return ui.column([
    ui.text(state.error.message, 'error'),
    ui.row([ui.button('retry', tr('tryAgain'), { icon: 'refresh-cw', variant: 'primary' }), ui.button('ask-agent', tr('askAgent'), { icon: 'wand-sparkles' })], { gap: 'small', wrap: true }),
  ]);
}

function loginCodeBlock(tool) {
  if (!state.loginCode || state.loginCode.tool !== tool) return null;
  return ui.column([
    ui.text(tool === 'github' ? tr('ghLoginCodeTitle') : tr('vercelLoginCodeTitle'), 'muted'),
    ui.text(state.loginCode.code, 'title'),
    ui.row(
      [ui.button('copy-code', tr('copyCode'), { icon: 'copy' }), ui.button('open-code-url', tool === 'github' ? tr('openGithub') : tr('openVercel'), { icon: 'external-link' })],
      { gap: 'small', wrap: true },
    ),
    ui.text(tr('ghLoginWaiting'), 'muted'),
  ]);
}

function body() {
  // While a step failed, its normal action button is replaced by the error block's "Try again" —
  // showing both would just be the same action twice.
  const failed = state.error?.step === state.step;
  switch (state.step) {
    case null:
      return ui.spinner();
    case 'no-pane':
      return ui.text(tr('noPane'), 'muted');
    case 'no-project':
      return ui.column([ui.text(tr('noProjectTitle'), 'title'), ui.text(tr('noProjectBody', { root: state.root ?? '' }), 'muted')]);
    case 'tools':
      return ui.column([
        ui.text(tr('toolsBody'), 'muted'),
        state.running
          ? ui.column([ui.spinner(tr('installingTools')), ...state.progressLines.map((l) => ui.text(l, 'small'))])
          : !failed && ui.button('install-tools', tr('installTools'), { icon: 'download', variant: 'primary' }),
        errorBlock(),
      ]);
    case 'gh-login':
      return ui.column([
        ui.text(tr('ghLoginBody'), 'muted'),
        state.loginCode
          ? loginCodeBlock('github')
          : state.running
            ? ui.spinner(tr('ghLoggingIn'))
            : !failed && ui.button('gh-login-start', tr('ghLoginButton'), { icon: 'rocket', variant: 'primary' }),
        errorBlock(),
      ]);
    case 'gh-save':
      return ui.column([
        state.gh.username ? ui.text(tr('ghLoggedInAs', { username: state.gh.username }), 'muted') : null,
        ui.text(tr('githubSaveBody'), 'muted'),
        ui.toggle('repo-public-toggle', tr('publicRepo'), state.publicRepo),
        state.running ? ui.spinner(tr('savingToGithub')) : !failed && ui.button('gh-save', tr('githubSaveButton'), { icon: 'git-branch', variant: 'primary' }),
        errorBlock(),
      ]);
    case 'vercel-login':
      return ui.column([
        state.vercelFallback
          ? ui.column([
              ui.text(tr('vercelFallbackBody'), 'muted'),
              ui.row([ui.button('vercel-login-terminal', tr('vercelLoginTerminal'), { icon: 'terminal', variant: 'primary' }), ui.button('vercel-login-check', tr('checkAgain'), { icon: 'refresh-cw' })], {
                gap: 'small',
                wrap: true,
              }),
            ])
          : ui.column([
              ui.text(tr('vercelLoginBody'), 'muted'),
              state.loginCode
                ? loginCodeBlock('vercel')
                : state.running
                  ? ui.spinner(tr('vercelLoggingIn'))
                  : !failed && ui.button('vercel-login-start', tr('vercelLoginButton'), { icon: 'cloud', variant: 'primary' }),
            ]),
        errorBlock(),
      ]);
    case 'env':
      return ui.column([
        ui.text(tr('envBody'), 'muted'),
        ...Object.entries(state.envDiscovery?.vars ?? {}).map(([key, entry]) =>
          ui.row([ui.toggle(`env:${key}`, entry.isLocal ? `${key} — ${tr('envLocal')}` : key, state.envSelection[key] ?? false)], { gap: 'small' }),
        ),
        state.running
          ? ui.spinner(tr('addingEnv'))
          : !failed && ui.row([ui.button('env-add', tr('envAdd'), { icon: 'upload', variant: 'primary' }), ui.button('env-skip', tr('envSkip'), { icon: 'x' })], { gap: 'small', wrap: true }),
        errorBlock(),
      ]);
    case 'deploy':
      return ui.column([
        ui.text(tr('deployBody'), 'muted'),
        state.running
          ? ui.column([ui.spinner(tr('deploying')), ui.text(state.deployLog.join('\n'), 'code')])
          : !failed && ui.button('deploy-start', tr('deployButton'), { icon: 'rocket', variant: 'primary' }),
        errorBlock(),
      ]);
    case 'launched':
      return ui.column([
        ui.badge(tr('launchedTitle'), 'success'),
        ui.text(tr('launchedBody'), 'muted'),
        ui.text(state.launched?.url ?? '', 'code'),
        ui.row([ui.button('open-site', tr('openSite'), { icon: 'external-link', variant: 'primary' }), ui.button('copy-link', tr('copyLink'), { icon: 'copy' })], { gap: 'small', wrap: true }),
        ui.row(
          [
            state.running || state.error ? null : ui.button('update-site', tr('updateSite'), { icon: 'refresh-cw' }),
            state.repoUrl ? ui.button('open-repo', tr('openRepo'), { icon: 'git-branch' }) : null,
          ],
          { gap: 'small', wrap: true },
        ),
        state.running ? ui.spinner(tr('updatingSite')) : null,
        state.gitConnected === false ? ui.text(tr('autoDeployHint'), 'small') : null,
        state.launched?.time ? ui.text(tr('lastPublished', { time: relativeTime(state.launched.time) }), 'small') : null,
        errorBlock(),
      ]);
    default:
      return null;
  }
}

async function render() {
  if (!state.panelOpen) return;
  const framework = state.inspect?.framework;
  await plugin.setPanel(
    ui.column([
      ui.row([ui.text(tr('title'), 'title'), ui.button('refresh', tr('refresh'), { icon: 'refresh-cw', variant: 'ghost' })], { gap: 'small' }),
      state.root ? ui.text(state.root, 'small') : null,
      state.inspect && state.step !== 'no-project' ? ui.text(framework ? tr('frameworkLine', { framework }) : tr('frameworkUnknown'), 'muted') : null,
      state.step && state.step !== 'no-project' && state.step !== 'no-pane' ? stepsChecklist() : null,
      state.step ? ui.divider() : null,
      body(),
    ]),
  );
}

// -- context / commands -----------------------------------------------------------------------

function projectCwdFromContext(context) {
  return context?.pane?.cwd ?? null;
}

async function openForContext(context, overridePath) {
  await plugin.showPanel();
  state.panelOpen = true;
  const cwd = overridePath ?? projectCwdFromContext(context) ?? plugin.context?.pane?.cwd ?? null;
  await openProject(cwd);
}

async function openValidAbsoluteDir(candidate, context) {
  if (typeof candidate !== 'string' || !path.isAbsolute(candidate) || !existsSync(candidate) || !statSync(candidate).isDirectory()) {
    await plugin.notify(tr('noProjectTitle'), 'error');
    return;
  }
  await openForContext(context, candidate);
}

plugin
  .onPanelOpen(async (context) => {
    state.panelOpen = true;
    if (!state.root) await openProject(projectCwdFromContext(context));
    else await render();
  })
  .onPanelClose(() => {
    state.panelOpen = false;
  })
  .onContextChange(async (context) => {
    if (!state.panelOpen || state.running) return;
    const cwd = projectCwdFromContext(context);
    if (!cwd) return;
    const root = await findProjectRoot(cwd);
    if (root !== state.root) await openProject(cwd);
  })
  .onEvent('refresh', () => (state.root ? resume() : openProject(projectCwdFromContext(plugin.context))))
  .onEvent('install-tools', installTools)
  .onEvent('gh-login-start', startGithubLogin)
  .onEvent('copy-code', async () => {
    if (state.loginCode) await copyToClipboard(state.loginCode.code);
    await plugin.notify(tr('codeCopied'), 'success');
  })
  .onEvent('open-code-url', async () => {
    if (state.loginCode) await plugin.openUrl(state.loginCode.url);
  })
  .onEvent('repo-public-toggle', (event) => {
    state.publicRepo = Boolean(event.value);
  })
  .onEvent('gh-save', startGithubSave)
  .onEvent('vercel-login-start', startVercelLogin)
  .onEvent('vercel-login-terminal', openVercelLoginTerminal)
  .onEvent('vercel-login-check', checkVercelLoginAgain)
  .onEvent('env-add', startEnvAdd)
  .onEvent('env-skip', skipEnv)
  .onEvent('deploy-start', startDeploy)
  .onEvent('update-site', startUpdateSite)
  .onEvent('open-site', async () => state.launched && plugin.openUrl(state.launched.url))
  .onEvent('copy-link', async () => {
    if (state.launched) await copyToClipboard(state.launched.url);
    await plugin.notify(tr('linkCopied'), 'success');
  })
  .onEvent('open-repo', async () => state.repoUrl && plugin.openUrl(state.repoUrl))
  .onEvent('retry', () => state.lastAction?.run())
  .onEvent('ask-agent', askAgentToFix)
  .onAnyEvent((event) => {
    const match = /^env:(.+)$/.exec(event.element ?? '');
    if (match) state.envSelection[match[1]] = Boolean(event.value);
  })
  .command('launch.open', ({ context, args }) => openForContext(context, typeof args?.path === 'string' ? args.path : undefined))
  .command('launch.redeploy', async ({ context }) => {
    await openForContext(context);
    if (state.step === 'launched') await startUpdateSite();
  })
  .onUrl('open', ({ query, context }) => openValidAbsoluteDir(query?.path, context))
  .start();
