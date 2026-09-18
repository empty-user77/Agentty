// Launch for Agentty: takes a project from a folder on disk to a live URL — GitHub login, save the
// project to a GitHub repo, Vercel login, the database (Supabase, when the project uses one),
// environment variables, production deploy, done. Every step shows one primary button; nothing runs
// until the user presses it.

import path from 'node:path';
import { existsSync, statSync } from 'node:fs';
import { createPlugin, ui } from './agentty-plugin.mjs';
import { run, tailLines } from './lib/exec.mjs';
import { ensureGh, ensureSupabase, ensureVercel, findGh, findVercel } from './lib/tools.mjs';
import { sanitizeRepoName, supabaseRegionForTimeZone } from './lib/parse.mjs';
import {
  SupabaseError,
  createSupabaseProject,
  generateDbPassword,
  inspectSupabase,
  pushMigrations,
  saveDbPassword,
  supabaseLogin,
  supabaseOrgs,
  supabaseProjects,
  supabasePublicKey,
  waitUntilHealthy,
  writeSupabaseEnv,
} from './lib/supabase.mjs';
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
    needsRedeploy: 'Press "Update site" to apply the new settings to the live site.',
    cancel: 'Cancel',
    stepSupabase: 'Database',
    sbIntroBody: 'This project uses Supabase for its data. Connect it to a Supabase project (there is a free plan) so the live site has a real database.',
    sbWantedBody: 'Supabase gives your project a real online database and sign-in, with a free plan. Launch connects it for you.',
    sbConnect: 'Connect Supabase',
    sbConnecting: 'Connecting to Supabase…',
    sbSkip: 'Skip for now',
    sbLoginTitle: 'Log in to Supabase in your browser. It will show you a verification code — enter it here.',
    sbOpenLogin: 'Open Supabase login',
    sbCodePlaceholder: 'Verification code',
    sbCodeSubmit: 'Confirm code',
    sbCodeChecking: 'Checking the code…',
    sbPickBody: 'Choose the Supabase project to use, or create a new one.',
    sbOrgLabel: 'Organization',
    sbCreate: 'Create a new project',
    sbCreateHint: 'A new project "{name}" in {region}. Its database password is generated for you and saved only in .env.local on this computer.',
    sbCreating: 'Creating your Supabase project… (a minute or two)',
    sbStarting: 'Supabase is starting the project… ({status})',
    sbUsing: 'Connecting to {name}…',
    sbConnectedTitle: 'Database connected',
    sbConnectedBody: 'Connected to {name}. The project address and public key were saved to {file} as {url} and {key}. Secret keys are never used.',
    sbContinue: 'Continue',
    sbAskAgent: 'Ask the agent to use the database',
    sbAskedAgent: 'Asked the agent to connect the app to Supabase…',
    sbMigrateBody: 'Your project has database changes that are not applied to Supabase yet ({count}).',
    sbMigrate: 'Apply database changes',
    sbMigrating: 'Applying database changes…',
    sbDbPasswordBody:
      "Launch doesn't know this project's database password (you chose it when you created the project on Supabase). Enter it to apply the database changes — it is saved only in .env.local on this computer.",
    sbDbPasswordPlaceholder: 'Database password',
    sbDbPasswordSave: 'Save and apply',
    sbAddDatabase: 'Connect a database (Supabase)',
    sbErrFreeLimit: 'Your Supabase account already has the maximum number of free projects. Choose one of your existing projects, or pause or delete one on supabase.com first.',
    sbErrNoOrg: 'Your Supabase account has no organization yet. Create one on supabase.com (it takes a few seconds), then try again.',
    sbErrStarting: 'Supabase is still starting the project. Wait a minute, then press "Try again".',
    sbErrDbPassword: "The database password didn't work. You can check or reset it on supabase.com → Project Settings → Database.",
    sbErrLogin: "The Supabase login didn't finish. Try again and enter the code shown in the browser.",
    sbPrompt:
      'This project is now connected to a hosted Supabase project by Agentty Launch. The project URL and public (anon) key are in {file} as {url} and {key}.\n\nPlease:\n1. Add @supabase/supabase-js and one small client module that reads those two variables.\n2. Move the sample / in-memory data to Supabase tables: write SQL migrations in supabase/migrations/<timestamp>_<name>.sql that create the tables, enable row level security on every table and add policies that fit the app.\n3. Never use or ask for the service_role key, and never put keys in code — only in the env file. Keep .env.example listing the names without values.\n4. Keep `npm run build` passing.\n\nWhen the migrations are ready, tell me in plain words to open Launch and press "Apply database changes".',
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
    needsRedeploy: '새 설정을 실제 사이트에 반영하려면 "사이트 업데이트"를 눌러 주세요.',
    cancel: '취소',
    stepSupabase: '데이터베이스',
    sbIntroBody: '이 프로젝트는 데이터 저장에 Supabase를 사용합니다. Supabase 프로젝트(무료 플랜 있음)에 연결하면 실제 사이트에서도 진짜 데이터베이스가 동작합니다.',
    sbWantedBody: 'Supabase는 프로젝트에 실제 온라인 데이터베이스와 로그인 기능을 제공하며 무료 플랜이 있습니다. 연결은 Launch가 대신 해 드려요.',
    sbConnect: 'Supabase 연결',
    sbConnecting: 'Supabase에 연결하는 중…',
    sbSkip: '나중에 하기',
    sbLoginTitle: '브라우저에서 Supabase에 로그인하면 인증 코드가 표시됩니다 — 그 코드를 여기에 입력해 주세요.',
    sbOpenLogin: 'Supabase 로그인 열기',
    sbCodePlaceholder: '인증 코드',
    sbCodeSubmit: '코드 확인',
    sbCodeChecking: '코드를 확인하는 중…',
    sbPickBody: '사용할 Supabase 프로젝트를 고르거나 새로 만드세요.',
    sbOrgLabel: '조직',
    sbCreate: '새 프로젝트 만들기',
    sbCreateHint: '{region} 지역에 새 프로젝트 "{name}"을(를) 만듭니다. 데이터베이스 비밀번호는 자동으로 생성되어 이 컴퓨터의 .env.local 에만 저장됩니다.',
    sbCreating: 'Supabase 프로젝트를 만드는 중… (1~2분 걸려요)',
    sbStarting: 'Supabase가 프로젝트를 시작하는 중… ({status})',
    sbUsing: '{name}에 연결하는 중…',
    sbConnectedTitle: '데이터베이스 연결 완료',
    sbConnectedBody: '{name}에 연결했습니다. 프로젝트 주소와 공개 키를 {file} 에 {url}, {key} (으)로 저장했습니다. 비밀 키는 절대 사용하지 않습니다.',
    sbContinue: '계속',
    sbAskAgent: '에이전트에게 데이터베이스 연결 맡기기',
    sbAskedAgent: '에이전트에게 Supabase 연결을 요청했습니다…',
    sbMigrateBody: '아직 Supabase에 적용되지 않은 데이터베이스 변경이 있습니다 ({count}개).',
    sbMigrate: '데이터베이스 변경 적용',
    sbMigrating: '데이터베이스 변경을 적용하는 중…',
    sbDbPasswordBody:
      'Launch는 이 프로젝트의 데이터베이스 비밀번호를 모릅니다 (Supabase에서 프로젝트를 만들 때 직접 정한 값). 데이터베이스 변경을 적용하려면 입력해 주세요 — 이 컴퓨터의 .env.local 에만 저장됩니다.',
    sbDbPasswordPlaceholder: '데이터베이스 비밀번호',
    sbDbPasswordSave: '저장하고 적용',
    sbAddDatabase: '데이터베이스 연결 (Supabase)',
    sbErrFreeLimit: 'Supabase 계정의 무료 프로젝트 수가 이미 최대입니다. 기존 프로젝트 중 하나를 고르거나, supabase.com 에서 하나를 일시정지/삭제한 뒤 다시 시도해 주세요.',
    sbErrNoOrg: 'Supabase 계정에 아직 조직이 없습니다. supabase.com 에서 조직을 만든 뒤(몇 초면 됩니다) 다시 시도해 주세요.',
    sbErrStarting: 'Supabase가 아직 프로젝트를 시작하는 중입니다. 1분쯤 뒤에 "다시 시도"를 눌러 주세요.',
    sbErrDbPassword: '데이터베이스 비밀번호가 맞지 않습니다. supabase.com → Project Settings → Database 에서 확인하거나 재설정할 수 있어요.',
    sbErrLogin: 'Supabase 로그인이 끝나지 않았습니다. 다시 시도해서 브라우저에 표시된 코드를 입력해 주세요.',
    sbPrompt:
      'Agentty Launch가 이 프로젝트를 Supabase 호스팅 프로젝트에 연결했습니다. 프로젝트 URL과 공개(anon) 키는 {file} 에 {url}, {key} (으)로 들어 있습니다.\n\n다음을 해 주세요:\n1. @supabase/supabase-js 를 추가하고, 위 두 변수를 읽는 작은 클라이언트 모듈을 하나 만듭니다.\n2. 샘플/메모리 데이터를 Supabase 테이블로 옮깁니다: supabase/migrations/<timestamp>_<name>.sql 에 테이블을 만드는 SQL 마이그레이션을 작성하고, 모든 테이블에 row level security 를 켜고 앱에 맞는 정책을 추가합니다.\n3. service_role 키는 절대 사용하거나 요구하지 말고, 키를 코드에 넣지 마세요 — env 파일에만 둡니다. .env.example 에는 값 없이 이름만 적어 둡니다.\n4. `npm run build` 가 계속 통과해야 합니다.\n\n마이그레이션이 준비되면 Launch를 열어 "데이터베이스 변경 적용"을 누르라고 쉬운 말로 알려 주세요.',
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
    cancel: 'キャンセル',
    stepSupabase: 'データベース',
    sbConnect: 'Supabase に接続',
    sbSkip: '後で',
    sbOpenLogin: 'Supabase ログインを開く',
    sbCodePlaceholder: '認証コード',
    sbCodeSubmit: 'コードを確認',
    sbOrgLabel: '組織',
    sbCreate: '新しいプロジェクトを作成',
    sbContinue: '続ける',
    sbAskAgent: 'エージェントにデータベース接続を依頼',
    sbMigrate: 'データベースの変更を適用',
    sbDbPasswordPlaceholder: 'データベースのパスワード',
    sbDbPasswordSave: '保存して適用',
    sbAddDatabase: 'データベースを接続 (Supabase)',
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
    cancel: '取消',
    stepSupabase: '数据库',
    sbConnect: '连接 Supabase',
    sbSkip: '暂时跳过',
    sbOpenLogin: '打开 Supabase 登录',
    sbCodePlaceholder: '验证码',
    sbCodeSubmit: '确认验证码',
    sbOrgLabel: '组织',
    sbCreate: '创建新项目',
    sbContinue: '继续',
    sbAskAgent: '请智能体接入数据库',
    sbMigrate: '应用数据库变更',
    sbDbPasswordPlaceholder: '数据库密码',
    sbDbPasswordSave: '保存并应用',
    sbAddDatabase: '连接数据库 (Supabase)',
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

// 'supabase' comes before 'env' so the keys it writes are offered to the live site right after.
const STEP_ORDER = ['tools', 'gh-login', 'gh-save', 'vercel-login', 'supabase', 'env', 'deploy'];

function freshSupabaseState() {
  return {
    bin: null,
    info: null, // inspectSupabase()
    phase: 'connect', // 'connect' | 'pick' | 'connected' | 'migrate' | 'db-password'
    wanted: false, // "Connect a database" pressed in a project that doesn't use Supabase yet
    login: null, // running `supabase login`: { submitCode, cancel, done }
    loginUrl: null,
    codeDraft: '',
    codeSent: false,
    passwordDraft: '',
    projects: [],
    orgs: [],
    orgId: null,
    created: null, // { ref, name } once `projects create` succeeded, so a retry never creates a second one
    connectedName: null,
    migrationsSkipped: false,
    thenUpdate: false, // "Update site" was interrupted by pending migrations: continue with it after applying
  };
}

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
  needsRedeploy: false,
  sb: freshSupabaseState(),
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
  state.sb = freshSupabaseState();
  state.needsRedeploy = false;
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
  state.sb.info = await inspectSupabase(state.root, state.inspect);
  if ((state.sb.info.used || state.sb.wanted) && !state.sb.info.configured && !state.saved?.supabaseSkipped) {
    if (!['pick', 'connected'].includes(state.sb.phase)) state.sb.phase = 'connect';
    state.step = 'supabase';
    return render();
  }
  if (pendingMigrations().length > 0 && !state.sb.migrationsSkipped) {
    if (state.sb.phase !== 'db-password') state.sb.phase = 'migrate';
    state.step = 'supabase';
    return render();
  }
  state.envDiscovery = await discoverEnvVars(state.root);
  if (state.envDiscovery.files.length > 0 && !state.saved?.envVarsDone) {
    // Values that only make sense on this computer start unselected.
    state.envSelection = Object.fromEntries(Object.entries(state.envDiscovery.vars).map(([k, entry]) => [k, !entry.isLocal]));
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
    state.saved = await saveProject(dataDir(), state.root, { envVarsDone: true });
    // The live site only sees new variables after the next deploy.
    if (state.launched) state.needsRedeploy = true;
    await resume();
  });
}

function skipEnv() {
  return runStep('env', '', '', async () => {
    state.saved = await saveProject(dataDir(), state.root, { envVarsDone: true });
    await resume();
  });
}

// -- Supabase ---------------------------------------------------------------------------------

const SB_ERROR_KEYS = { 'free-limit': 'sbErrFreeLimit', 'no-org': 'sbErrNoOrg', starting: 'sbErrStarting', 'db-password': 'sbErrDbPassword', login: 'sbErrLogin' };

/** Migration files in the project that Launch has not applied to the hosted database yet. */
function pendingMigrations() {
  const info = state.sb.info;
  if (!info?.configured) return [];
  const applied = state.saved?.supabaseMigrations ?? [];
  return info.migrations.filter((file) => !applied.includes(file));
}

function supabaseRegion() {
  return supabaseRegionForTimeZone(Intl.DateTimeFormat().resolvedOptions().timeZone);
}

/** A Launch step for the database: account problems get a plain-language message instead of CLI output. */
function supabaseStep(label, command, fn) {
  return runStep('supabase', label, command, async () => {
    try {
      await fn();
    } catch (err) {
      const key = SB_ERROR_KEYS[err?.kind];
      throw key ? new Error(tr(key)) : err;
    } finally {
      state.sb.login = null;
      state.sb.loginUrl = null;
      state.sb.codeDraft = '';
      state.sb.codeSent = false;
    }
  });
}

/** Installs the CLI if needed and logs in (link in the browser, verification code typed into the panel). Resolves the project list. */
async function ensureSupabaseSession() {
  state.sb.bin = state.sb.bin ?? (await ensureSupabase(dataDir(), { log: progress }));
  let listing = await supabaseProjects(state.sb.bin, state.root);
  if (listing.loggedIn) return listing;
  state.sb.login = supabaseLogin(state.sb.bin, state.root, {
    onLink: async (url) => {
      state.sb.loginUrl = url;
      await render();
      try {
        await plugin.openUrl(url);
      } catch {
        // The "Open Supabase login" button is still there.
      }
    },
  });
  const result = await state.sb.login.done;
  if (result.ok) listing = await supabaseProjects(state.sb.bin, state.root);
  if (!listing.loggedIn) throw new SupabaseError('supabase login did not finish', 'login');
  return listing;
}

function startSupabaseConnect() {
  return supabaseStep(tr('sbConnecting'), 'supabase login / supabase projects list', async () => {
    const listing = await ensureSupabaseSession();
    state.sb.projects = listing.projects;
    state.sb.orgs = await supabaseOrgs(state.sb.bin, state.root);
    state.sb.orgId = state.sb.orgs.find((o) => o.id === state.sb.orgId)?.id ?? state.sb.orgs[0]?.id ?? null;
    state.sb.phase = 'pick';
  });
}

async function submitSupabaseCode(value) {
  const code = String(value ?? state.sb.codeDraft ?? '').trim();
  if (!code || !state.sb.login || state.sb.codeSent) return;
  state.sb.codeSent = true;
  state.sb.login.submitCode(code);
  await render();
}

/** Reads the public key of `ref` and writes it, with the project URL, where the app and the env step find them. */
async function finishSupabaseConnect(ref, name) {
  const info = state.sb.info;
  const publicKey = await supabasePublicKey(state.sb.bin, state.root, ref);
  await ensureGitignore(state.root); // `.env*` must be ignored before a key lands in one
  await writeSupabaseEnv(state.root, { envFile: info.envFile, envNames: info.envNames, ref, publicKey });
  // New variables: the environment step comes back so they reach the live site too.
  state.saved = await saveProject(dataDir(), state.root, { supabaseRef: ref, supabaseSkipped: false, envVarsDone: false });
  state.sb.created = null;
  state.sb.connectedName = name;
  state.sb.info = await inspectSupabase(state.root, state.inspect);
  state.sb.phase = 'connected';
}

function useSupabaseProject(ref) {
  const project = state.sb.projects.find((p) => p.ref === ref);
  if (!project || state.running) return null;
  return supabaseStep(tr('sbUsing', { name: project.name }), 'supabase projects api-keys', () => finishSupabaseConnect(project.ref, project.name));
}

function createSupabase() {
  // The command shown to the user and to "Ask the agent" never carries the generated password.
  return supabaseStep(tr('sbCreating'), 'supabase projects create', async () => {
    if (!state.sb.created) {
      if (!state.sb.orgId) throw new SupabaseError('no Supabase organization', 'no-org');
      const name = sanitizeRepoName(path.basename(state.root));
      const dbPassword = generateDbPassword();
      const ref = await createSupabaseProject(state.sb.bin, state.root, { name, orgId: state.sb.orgId, region: supabaseRegion(), dbPassword });
      state.sb.created = { ref, name };
      await ensureGitignore(state.root);
      await saveDbPassword(state.root, dbPassword);
    }
    const { ref, name } = state.sb.created;
    await waitUntilHealthy(state.sb.bin, state.root, ref, { onTick: (status) => progress(tr('sbStarting', { status })) });
    await finishSupabaseConnect(ref, name);
  });
}

function skipSupabase() {
  return supabaseStep('', '', async () => {
    state.sb.wanted = false;
    state.saved = await saveProject(dataDir(), state.root, { supabaseSkipped: true });
    await resume();
  });
}

/** Only reachable from the deploy / launched screens, so everything before the database step is known to be done. */
async function addDatabase() {
  state.sb.wanted = true;
  state.sb.phase = 'connect';
  state.error = null;
  state.step = 'supabase';
  await render();
  state.saved = await saveProject(dataDir(), state.root, { supabaseSkipped: false });
}

async function askAgentToUseSupabase() {
  const info = state.sb.info;
  await plugin.injectPrompt({
    target: 'ask',
    cwd: state.root,
    title: tr('title'),
    text: tr('sbPrompt', { file: info.envFile, url: info.envNames.url, key: info.envNames.key }),
  });
  await plugin.notify(tr('sbAskedAgent'), 'info');
}

async function applySupabaseMigrations() {
  if (!state.sb.info?.hasDbPassword) {
    state.sb.phase = 'db-password';
    return render();
  }
  await supabaseStep(tr('sbMigrating'), 'supabase link / supabase db push', async () => {
    await ensureSupabaseSession();
    try {
      await pushMigrations(state.sb.bin, state.root, state.sb.info.ref);
    } catch (err) {
      if (err?.kind === 'db-password') state.sb.phase = 'db-password';
      throw err;
    }
    state.saved = await saveProject(dataDir(), state.root, { supabaseMigrations: state.sb.info.migrations });
    state.sb.phase = 'connect';
    await resume();
  });
  const thenUpdate = state.sb.thenUpdate;
  state.sb.thenUpdate = false;
  if (thenUpdate && !state.error && state.step === 'launched') await startUpdateSite();
}

async function submitDbPassword(value) {
  const password = String(value ?? state.sb.passwordDraft ?? '').trim();
  if (!password || state.running) return;
  state.sb.passwordDraft = '';
  await ensureGitignore(state.root);
  await saveDbPassword(state.root, password);
  state.sb.info = await inspectSupabase(state.root, state.inspect);
  state.sb.phase = 'migrate';
  await applySupabaseMigrations();
}

function skipSupabaseMigrations() {
  state.sb.migrationsSkipped = true;
  state.sb.thenUpdate = false;
  state.sb.phase = 'connect';
  return resume();
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
  state.needsRedeploy = false;
}

function startDeploy() {
  return runStep('deploy', tr('deploying'), 'vercel deploy --prod --yes', async () => {
    await doDeploy();
    state.step = 'launched';
    await plugin.notify(tr('launchedTitle'), 'success');
    await render();
  });
}

async function startUpdateSite() {
  // Database changes the agent wrote since the last visit go first: the new code expects them.
  state.sb.info = await inspectSupabase(state.root, state.inspect);
  if (pendingMigrations().length > 0 && !state.sb.migrationsSkipped) {
    state.sb.thenUpdate = true;
    state.sb.phase = 'migrate';
    state.step = 'supabase';
    return render();
  }
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
    // Only projects with a database see this row.
    state.step === 'supabase' || state.sb.info?.used || state.sb.info?.configured ? { id: 'supabase', label: tr('stepSupabase') } : null,
    { id: 'env', label: tr('stepEnv') },
    { id: 'deploy', label: tr('stepDeploy') },
  ].filter(Boolean);
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

/** "Connect a database" for projects that don't use Supabase (yet), or skipped it earlier. */
function addDatabaseButton() {
  if (state.running || state.error || state.sb.info?.configured) return null;
  return ui.button('sb-add', tr('sbAddDatabase'), { icon: 'database' });
}

function supabaseLoginBlock() {
  return ui.column([
    ui.text(tr('sbLoginTitle'), 'muted'),
    ui.button('sb-open-login', tr('sbOpenLogin'), { icon: 'external-link' }),
    state.sb.codeSent
      ? ui.spinner(tr('sbCodeChecking'))
      : ui.row(
          [ui.input('sb-code', { placeholder: tr('sbCodePlaceholder'), value: state.sb.codeDraft }), ui.button('sb-code-submit', tr('sbCodeSubmit'), { icon: 'circle-check', variant: 'primary' })],
          { gap: 'small', wrap: true },
        ),
    ui.button('sb-login-cancel', tr('cancel'), { icon: 'x', variant: 'ghost' }),
  ]);
}

function supabaseBody(failed) {
  const sb = state.sb;
  if (state.running) {
    return ui.column([sb.loginUrl ? supabaseLoginBlock() : ui.spinner(state.busyLabel), ...state.progressLines.map((l) => ui.text(l, 'small'))]);
  }
  switch (sb.phase) {
    case 'pick':
      return ui.column([
        ui.text(tr('sbPickBody'), 'muted'),
        sb.projects.length > 0
          ? ui.list(
              'sb-projects',
              sb.projects.map((p) => ({ id: p.ref, title: p.name, subtitle: p.region, icon: 'database' })),
            )
          : null,
        sb.orgs.length > 1
          ? ui.column(
              [
                ui.text(tr('sbOrgLabel'), 'small'),
                ui.choice(
                  'sb-org',
                  sb.orgs.map((o) => ({ value: o.id, label: o.name || o.id })),
                  sb.orgId ?? '',
                ),
              ],
              { gap: 'small' },
            )
          : null,
        ui.text(tr('sbCreateHint', { name: sanitizeRepoName(path.basename(state.root ?? '')), region: supabaseRegion() }), 'small'),
        !failed && ui.button('sb-create', tr('sbCreate'), { icon: 'square-plus', variant: sb.projects.length > 0 ? 'secondary' : 'primary' }),
        errorBlock(),
      ]);
    case 'connected':
      return ui.column([
        ui.badge(tr('sbConnectedTitle'), 'success'),
        ui.text(tr('sbConnectedBody', { name: sb.connectedName ?? '', file: sb.info.envFile, url: sb.info.envNames.url, key: sb.info.envNames.key }), 'muted'),
        ui.row(
          [
            ui.button('sb-continue', tr('sbContinue'), { icon: 'play', variant: 'primary' }),
            // Keys alone don't make the app use the database: offer to have the agent wire it up.
            !sb.info.hasClient || sb.info.migrations.length === 0 ? ui.button('sb-ask-agent', tr('sbAskAgent'), { icon: 'wand-sparkles' }) : null,
          ],
          { gap: 'small', wrap: true },
        ),
      ]);
    case 'migrate':
      return ui.column([
        ui.text(tr('sbMigrateBody', { count: pendingMigrations().length }), 'muted'),
        !failed && ui.row([ui.button('sb-migrate', tr('sbMigrate'), { icon: 'database', variant: 'primary' }), ui.button('sb-migrate-skip', tr('sbSkip'), { icon: 'x' })], { gap: 'small', wrap: true }),
        errorBlock(),
      ]);
    case 'db-password':
      return ui.column([
        // A wrong password lands back here: the message says why, the input takes the new one.
        failed ? ui.text(state.error.message, 'error') : null,
        ui.text(tr('sbDbPasswordBody'), 'muted'),
        ui.row([ui.input('sb-dbpass', { placeholder: tr('sbDbPasswordPlaceholder') }), ui.button('sb-dbpass-save', tr('sbDbPasswordSave'), { icon: 'database', variant: 'primary' })], { gap: 'small', wrap: true }),
        ui.button('sb-migrate-skip', tr('sbSkip'), { icon: 'x', variant: 'ghost' }),
      ]);
    default:
      return ui.column([
        ui.text(tr(sb.info?.used ? 'sbIntroBody' : 'sbWantedBody'), 'muted'),
        // Skipping stays possible after a failed attempt.
        ui.row([!failed && ui.button('sb-connect', tr('sbConnect'), { icon: 'database', variant: 'primary' }), ui.button('sb-skip', tr('sbSkip'), { icon: 'x' })], { gap: 'small', wrap: true }),
        errorBlock(),
      ]);
  }
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
    case 'supabase':
      return supabaseBody(failed);
    case 'deploy':
      return ui.column([
        ui.text(tr('deployBody'), 'muted'),
        state.running
          ? ui.column([ui.spinner(tr('deploying')), ui.text(state.deployLog.join('\n'), 'code')])
          : !failed && ui.button('deploy-start', tr('deployButton'), { icon: 'rocket', variant: 'primary' }),
        addDatabaseButton(),
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
            addDatabaseButton(),
          ],
          { gap: 'small', wrap: true },
        ),
        state.running ? ui.spinner(tr('updatingSite')) : null,
        state.needsRedeploy && !state.running ? ui.text(tr('needsRedeploy'), 'small') : null,
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
  .onEvent('sb-connect', startSupabaseConnect)
  .onEvent('sb-skip', skipSupabase)
  .onEvent('sb-add', addDatabase)
  .onEvent('sb-open-login', async () => state.sb.loginUrl && plugin.openUrl(state.sb.loginUrl))
  .onEvent('sb-code', (event) => {
    state.sb.codeDraft = String(event.value ?? '');
    if (event.event === 'submit') return submitSupabaseCode(event.value);
  })
  .onEvent('sb-code-submit', () => submitSupabaseCode())
  .onEvent('sb-login-cancel', () => state.sb.login?.cancel())
  .onEvent('sb-projects', (event) => (event.event === 'select' ? useSupabaseProject(event.item) : null))
  .onEvent('sb-org', (event) => {
    state.sb.orgId = String(event.value ?? '') || state.sb.orgId;
  })
  .onEvent('sb-create', createSupabase)
  .onEvent('sb-continue', () => {
    state.sb.phase = 'connect';
    return resume();
  })
  .onEvent('sb-ask-agent', askAgentToUseSupabase)
  .onEvent('sb-migrate', applySupabaseMigrations)
  .onEvent('sb-migrate-skip', skipSupabaseMigrations)
  .onEvent('sb-dbpass', (event) => {
    state.sb.passwordDraft = String(event.value ?? '');
    if (event.event === 'submit') return submitDbPassword(event.value);
  })
  .onEvent('sb-dbpass-save', () => submitDbPassword())
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
