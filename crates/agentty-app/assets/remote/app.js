// Agentty remote page, laid out as the app is: workspaces by group in a menu (a drawer on phones,
// a sidebar on wide screens), the chosen workspace's tabs across the top, the split panes of a tab
// in a drawer on the right, and the chosen terminal in the middle.
//
// Everything shown comes from terminals and agents, which anything running in them can set (a
// window title is an escape sequence away), so text only ever goes in through textContent: never
// innerHTML, never a URL, never a style string built from it. Colors pass a strict check first.
"use strict";

const STRINGS = {
  en: {
    toLatest: "Latest ↓",
    signIn: "Sign in", password: "Password",
    wrong: "Wrong password.", locked: "Too many wrong passwords. Try again in {s} s.", noPassword: "No password is set in Agentty yet.",
    failed: "Couldn't reach Agentty.", signOut: "Sign out", notify: "Notify me", notifyOn: "Notifying",
    noWorkspaces: "No workspaces are open in Agentty.", sleeping: "This workspace is asleep. Wake it up to start its terminals again.",
    send: "Send", promptHint: "Prompt for this terminal…", live: "Connected", offline: "Reconnecting…",
    micStart: "Dictate a prompt", micRecording: "Recording — tap to stop", micBusy: "Transcribing…", micDenied: "Microphone access was blocked.", micFailed: "Couldn't transcribe. Try again.", micTooBusy: "The Mac is busy transcribing. Try again in a moment.",
    capsLock: "Caps Lock is on.", hangulInput: "Passwords can only use English letters.", showPassword: "Show password", hidePassword: "Hide password",
    attach: "Attach images",attachRemove: "Remove image", attachFailed: "Couldn't attach this image.", attachWait: "Images are still uploading.",
    privacy: "Privacy Policy", terms: "Terms of Service", eula: "License Agreement", device: "Device", ip: "Tailnet IP", account: "Account", target: "Connecting to", secure: "Security", secureValue: "Tailscale (WireGuard) · HTTPS", browser: "Browser", shownOnly: "Shown here only; nothing is stored.",
    tooLong: "Too long.", busy: "Agentty is busy; try again.", finished: "Finished", asks: "Asks you", panes: "Split panes",
    home: "Home", needsYou: "Needs you", workingNow: "Working", workspacesTitle: "Workspaces", plugins: "Plugins",
    nothingWaiting: "Nothing is waiting for you.", nothingWorking: "No agent is working right now.", noPlugins: "No plugins installed.",
    wake: "Wake up", waking: "Waking…", newTab: "New tab", newIn: "New tab in {ws}", cancel: "Cancel",
    terminal: "Terminal", claude: "Claude Code", codex: "Codex", tooManyTabs: "Too many new tabs at once. Wait a minute.",
    newWorkspace: "New workspace", openHere: "Open this folder with", newFolder: "New folder", folderName: "Folder name", create: "Create", up: "Up", noFolders: "No folders here yet.", folderExists: "A folder with this name is already here.", badFolderName: "Use a plain folder name (no / or leading dot).", folderHint: "Workspaces opened from here live in {root} on the Mac.",
    open: "{n} open", asleep: "Asleep", running: "{n} running",
    pstate: { running: "Running", starting: "Starting", stopped: "Stopped", failed: "Stopped with an error", consent: "Waiting for your OK", off: "Off" },
    astate: { working: "Working", idle: "Idle", error: "Error" },
    chat: "Chat", chatHint: "Message the lead agent…", chatEmpty: "Write to the lead agent. It plans the work and starts workers on the Mac; their terminals are in the pane list.", leadWaits: "The lead is waiting for you in its terminal.",
  },
  ko: {
    toLatest: "최신 ↓",
    signIn: "로그인", password: "비밀번호",
    wrong: "비밀번호가 틀렸습니다.", locked: "비밀번호가 여러 번 틀렸습니다. {s}초 뒤에 다시 시도하세요.", noPassword: "Agentty에 아직 비밀번호가 설정되지 않았습니다.",
    failed: "Agentty에 연결하지 못했습니다.", signOut: "로그아웃", notify: "알림 받기", notifyOn: "알림 켜짐",
    noWorkspaces: "Agentty에 열린 작업공간이 없습니다.", sleeping: "쉬고 있는 작업공간입니다. 깨우면 터미널이 다시 시작됩니다.",
    send: "보내기", promptHint: "이 터미널에 보낼 프롬프트…", live: "연결됨", offline: "다시 연결하는 중…",
    micStart: "음성으로 입력", micRecording: "녹음 중 — 눌러서 멈추기", micBusy: "변환 중…", micDenied: "마이크 사용이 차단되었습니다.", micFailed: "변환하지 못했습니다. 다시 시도하세요.", micTooBusy: "Mac이 다른 음성을 변환하는 중입니다. 잠시 후 다시 시도하세요.",
    capsLock: "Caps Lock이 켜져 있습니다.", hangulInput: "비밀번호는 영어로만 설정 가능합니다.", showPassword: "비밀번호 보기", hidePassword: "비밀번호 숨기기",
    attach: "이미지 첨부",attachRemove: "이미지 빼기", attachFailed: "이 이미지를 첨부하지 못했습니다.", attachWait: "이미지를 아직 올리는 중입니다.",
    privacy: "개인정보처리방침", terms: "이용약관", eula: "라이선스 계약", device: "접속 기기", ip: "Tailnet IP", account: "계정", target: "접속 대상", secure: "보안", secureValue: "Tailscale(WireGuard) 암호화 · HTTPS", browser: "브라우저", shownOnly: "이 화면에 보여 주기만 하고 저장하지 않습니다.",
    tooLong: "너무 깁니다.", busy: "Agentty가 바쁩니다. 다시 시도하세요.", finished: "작업 완료", asks: "요청", panes: "분할창",
    home: "홈", needsYou: "응답 대기", workingNow: "작업 중", workspacesTitle: "작업공간", plugins: "플러그인",
    nothingWaiting: "기다리는 요청이 없습니다.", nothingWorking: "지금 작업 중인 에이전트가 없습니다.", noPlugins: "설치된 플러그인이 없습니다.",
    wake: "깨우기", waking: "깨우는 중…", newTab: "새 탭", newIn: "{ws}에 새 탭", cancel: "취소",
    terminal: "터미널", claude: "Claude Code", codex: "Codex", tooManyTabs: "새 탭을 너무 많이 열었습니다. 1분 뒤에 다시 시도하세요.",
    newWorkspace: "새 작업공간", openHere: "이 폴더를 열 도구", newFolder: "새 폴더", folderName: "폴더 이름", create: "만들기", up: "위로", noFolders: "아직 폴더가 없습니다.", folderExists: "같은 이름의 폴더가 이미 있습니다.", badFolderName: "/나 점(.)으로 시작하지 않는 폴더 이름을 쓰세요.", folderHint: "여기서 여는 작업공간은 Mac의 {root} 폴더 안에 있습니다.",
    open: "{n}개 열림", asleep: "쉬는 중", running: "{n}개 실행 중",
    pstate: { running: "실행 중", starting: "시작하는 중", stopped: "멈춤", failed: "오류로 멈춤", consent: "허용 대기", off: "꺼짐" },
    astate: { working: "작업 중", idle: "대기", error: "오류" },
    chat: "채팅", chatHint: "리드 에이전트에게 메시지…", chatEmpty: "리드 에이전트에게 말해 보세요. 작업을 계획하고 Mac에서 워커를 띄웁니다. 워커 터미널은 분할창 목록에 있습니다.", leadWaits: "리드가 터미널에서 응답을 기다립니다.",
  },
  ja: {
    toLatest: "最新へ ↓",
    signIn: "サインイン", password: "パスワード",
    wrong: "パスワードが違います。", locked: "パスワードの誤りが多すぎます。{s} 秒後に再試行してください。", noPassword: "Agentty にまだパスワードが設定されていません。",
    failed: "Agentty に接続できませんでした。", signOut: "サインアウト", notify: "通知を受け取る", notifyOn: "通知オン",
    noWorkspaces: "Agentty で開いているワークスペースはありません。", sleeping: "休止中のワークスペースです。起こすとターミナルがまた始まります。",
    send: "送信", promptHint: "このターミナルへのプロンプト…", live: "接続中", offline: "再接続しています…",
    micStart: "音声で入力", micRecording: "録音中 — タップで停止", micBusy: "文字起こし中…", micDenied: "マイクの使用がブロックされました。", micFailed: "文字起こしできませんでした。もう一度お試しください。", micTooBusy: "Mac が別の音声を文字起こし中です。少し待ってからもう一度お試しください。",
    capsLock: "Caps Lock がオンです。", hangulInput: "パスワードは英字でのみ設定できます。", showPassword: "パスワードを表示", hidePassword: "パスワードを隠す",
    attach: "画像を添付",attachRemove: "画像を外す", attachFailed: "この画像を添付できませんでした。", attachWait: "画像をまだアップロードしています。",
    privacy: "プライバシーポリシー", terms: "利用規約", eula: "ライセンス契約", device: "接続端末", ip: "Tailnet IP", account: "アカウント", target: "接続先", secure: "セキュリティ", secureValue: "Tailscale（WireGuard）暗号化 · HTTPS", browser: "ブラウザ", shownOnly: "表示するだけで、保存はしません。",
    tooLong: "長すぎます。", busy: "Agentty が混み合っています。もう一度お試しください。", finished: "完了", asks: "質問", panes: "分割ペイン",
    home: "ホーム", needsYou: "応答待ち", workingNow: "作業中", workspacesTitle: "ワークスペース", plugins: "プラグイン",
    nothingWaiting: "待っている質問はありません。", nothingWorking: "いま作業中のエージェントはありません。", noPlugins: "インストール済みのプラグインはありません。",
    wake: "起こす", waking: "起こしています…", newTab: "新しいタブ", newIn: "{ws} に新しいタブ", cancel: "キャンセル",
    terminal: "ターミナル", claude: "Claude Code", codex: "Codex", tooManyTabs: "新しいタブを開きすぎました。1 分後にもう一度お試しください。",
    newWorkspace: "新しいワークスペース", openHere: "このフォルダを開くツール", newFolder: "新しいフォルダ", folderName: "フォルダ名", create: "作成", up: "上へ", noFolders: "まだフォルダがありません。", folderExists: "同じ名前のフォルダがすでにあります。", badFolderName: "/ やドット(.)で始まらないフォルダ名にしてください。", folderHint: "ここで開くワークスペースは Mac の {root} フォルダの中にあります。",
    open: "{n} 件 開いています", asleep: "休止中", running: "{n} 件 実行中",
    pstate: { running: "実行中", starting: "起動中", stopped: "停止", failed: "エラーで停止", consent: "許可待ち", off: "オフ" },
    astate: { working: "作業中", idle: "待機", error: "エラー" },
    chat: "チャット", chatHint: "リードエージェントにメッセージ…", chatEmpty: "リードエージェントに書いてください。作業を計画し、Mac でワーカーを起動します。ワーカーのターミナルはペイン一覧にあります。", leadWaits: "リードがターミナルで返答を待っています。",
  },
  zh: {
    toLatest: "最新 ↓",
    signIn: "登录", password: "密码",
    wrong: "密码错误。", locked: "密码错误次数过多，请在 {s} 秒后重试。", noPassword: "Agentty 尚未设置密码。",
    failed: "无法连接 Agentty。", signOut: "退出登录", notify: "接收通知", notifyOn: "通知已开启",
    noWorkspaces: "Agentty 中没有打开的工作区。", sleeping: "这个工作区处于休眠状态。唤醒后终端会重新启动。",
    send: "发送", promptHint: "发送给此终端的提示…", live: "已连接", offline: "正在重新连接…",
    micStart: "语音输入", micRecording: "录音中 — 点按停止", micBusy: "转写中…", micDenied: "麦克风访问被阻止。", micFailed: "转写失败，请重试。", micTooBusy: "Mac 正在转写其他语音，请稍后再试。",
    capsLock: "大写锁定已开启。", hangulInput: "密码只能使用英文字母设置。", showPassword: "显示密码", hidePassword: "隐藏密码",
    attach: "附加图片",attachRemove: "移除图片", attachFailed: "无法附加这张图片。", attachWait: "图片仍在上传。",
    privacy: "隐私政策", terms: "服务条款", eula: "许可协议", device: "接入设备", ip: "Tailnet IP", account: "账号", target: "连接目标", secure: "安全", secureValue: "Tailscale（WireGuard）加密 · HTTPS", browser: "浏览器", shownOnly: "仅在此显示，不会保存。",
    tooLong: "太长了。", busy: "Agentty 正忙，请重试。", finished: "已完成", asks: "询问", panes: "分屏",
    home: "主页", needsYou: "等待回应", workingNow: "工作中", workspacesTitle: "工作区", plugins: "插件",
    nothingWaiting: "没有等待你的请求。", nothingWorking: "现在没有正在工作的代理。", noPlugins: "没有安装插件。",
    wake: "唤醒", waking: "正在唤醒…", newTab: "新标签页", newIn: "在 {ws} 中新建标签页", cancel: "取消",
    terminal: "终端", claude: "Claude Code", codex: "Codex", tooManyTabs: "新建的标签页太多了，请一分钟后再试。",
    newWorkspace: "新建工作区", openHere: "用以下工具打开此文件夹", newFolder: "新建文件夹", folderName: "文件夹名称", create: "创建", up: "上一级", noFolders: "这里还没有文件夹。", folderExists: "已有同名文件夹。", badFolderName: "请使用不含 / 且不以点(.)开头的文件夹名。", folderHint: "在这里打开的工作区位于 Mac 的 {root} 文件夹中。",
    open: "{n} 个已打开", asleep: "休眠中", running: "{n} 个运行中",
    pstate: { running: "运行中", starting: "正在启动", stopped: "已停止", failed: "因错误停止", consent: "等待你的允许", off: "已关闭" },
    astate: { working: "工作中", idle: "空闲", error: "错误" },
    chat: "聊天", chatHint: "给主控智能体发消息…", chatEmpty: "给主控智能体写消息。它会规划工作并在 Mac 上启动工作智能体，它们的终端在窗格列表中。", leadWaits: "主控智能体正在终端中等你回应。",
  },
};
const lang = (navigator.language || "en").slice(0, 2);
const T = STRINGS[lang] || STRINGS.en;
document.documentElement.lang = STRINGS[lang] ? lang : "en";

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text != null) node.textContent = String(text);
  return node;
};
const hex = (n) => "#" + ((Number(n) >>> 0) & 0xffffff).toString(16).padStart(6, "0");
const safeColor = (c) => (typeof c === "string" && /^#[0-9a-f]{6}$/i.test(c) ? c : null);
const wide = () => matchMedia("(min-width: 900px)").matches;

const DEFAULT_SIZE = 13;
const MIN_SIZE = 9;
const MAX_SIZE = 20;

const state = {
  workspaces: [],
  sessions: [],
  plugins: [],
  voice: false, // whether the Mac offers voice-to-text (a model is installed)
  previous: new Map(),
  // The first overview decides where to start; after that the page stays where the user is.
  started: false,
  workspace: null, // id; null: the home page
  // A tab asked for and not seen yet: { workspace, count } (its tab count when asked).
  pendingNew: null,
  // A workspace asked to wake up, until it has.
  waking: null,
  // The workspace the new-tab sheet is open for.
  sheetFor: null,
  // The folder sheet: the folder shown (names under the remote folder), what is in it, and a
  // workspace asked for there and not seen yet ({ ids, tabs }: what was there when asked).
  folderSheet: null,
  pendingFolder: null,
  tab: 0,
  pane: null,
  events: null,
  rows: [],
  lines: [],
  screen: null,
  // The terminal stays at its newest line as output comes in; off while the user reads back.
  follow: true,
  // Text at a fixed, readable size (the app's own by default; A− / A+ change it). The terminal
  // takes as many columns and lines as fit the page at that size while it is shown here.
  size: (() => {
    try {
      const saved = Number(localStorage.getItem("agentty.size"));
      if (saved >= MIN_SIZE && saved <= MAX_SIZE) return saved;
    } catch (_) { /* private mode */ }
    return DEFAULT_SIZE;
  })(),
  ctrl: false,
  // A chat's lead is shown as its chat (false: as its terminal).
  chatView: true,
  // The size last asked of the terminal being looked at ("pane:cols:rows").
  sized: null,
};

const session = (pane) => state.sessions.find((s) => s.pane === pane);

/// The chat of the tab on screen, if it is a chat tab.
function currentChat() {
  const ws = workspace(state.workspace);
  const tab = ws && ws.tabs[state.tab];
  return (tab && tab.chat) || null;
}

/// Whether the lead is on screen as its chat. While it waits on an approval or a question, its
/// terminal is shown instead: only there can that be answered.
function inChat() {
  const chat = currentChat();
  if (!chat || state.pane !== chat.lead || !state.chatView) return false;
  const lead = session(chat.lead);
  return !(lead && lead.needs_user);
}
const workspace = (id) => state.workspaces.find((w) => w.id === id);

async function api(path, body) {
  const options = body === undefined
    ? { credentials: "same-origin" }
    : { method: "POST", credentials: "same-origin", headers: { "Content-Type": "application/json", "X-Agentty": "1" }, body: JSON.stringify(body) };
  const response = await fetch(path, options);
  let data = {};
  try { data = await response.json(); } catch (_) { /* not JSON */ }
  return { status: response.status, data };
}

// ---- Sign in ----

let lockTimer = null;
function lockedFor(seconds) {
  clearInterval(lockTimer);
  let left = seconds;
  const button = $("login-button");
  const tick = () => {
    if (left <= 0) {
      clearInterval(lockTimer);
      button.disabled = false;
      $("login-error").textContent = "";
      return;
    }
    button.disabled = true;
    $("login-error").textContent = T.locked.replace("{s}", left);
    left -= 1;
  };
  tick();
  lockTimer = setInterval(tick, 1000);
}

async function signIn(event) {
  event.preventDefault();
  const input = $("password");
  const button = $("login-button");
  button.disabled = true;
  $("login-error").textContent = "";
  try {
    const { status, data } = await api("/api/login", { password: pw.real.join("") });
    clearPassword();
    input.blur();
    if (status === 200) return start();
    if (status === 429) return lockedFor(data.lockedFor || 60);
    $("login-error").textContent = status === 401 ? T.wrong : status === 409 ? T.noPassword : T.failed;
  } catch (_) {
    $("login-error").textContent = T.failed;
  }
  button.disabled = false;
}

// ---- Password box ----
//
// A text box the page masks itself, so the character just typed can show for a moment (as a
// phone's password box does) and the eye can show it all. The box shows `display()`; the
// password is `pw.real`. Each edit is read back as what changed between the two displays: the
// same number of characters, so a place in one is the same place in the other.

/// Hangul syllables and jamo: passwords are set in English only (the Mac refuses Korean too), so
/// Korean here only means the Korean keyboard is on.
const HANGUL = /[ᄀ-ᇿ㄰-㆏ꥠ-꥿가-퟿]/;
const BULLET = "•";
const pw = { real: [], shown: [], peek: -1, timer: null, revealed: false, capsLock: false, refused: false };

const display = () => pw.real.map((c, i) => (pw.revealed || i === pw.peek ? c : BULLET));

function showPassword(caret) {
  const input = $("password");
  pw.shown = display();
  input.value = pw.shown.join("");
  if (caret != null && document.activeElement === input) {
    const at = pw.shown.slice(0, caret).join("").length;
    input.setSelectionRange(at, at);
  }
}

/// Reads an edit of the box into `pw.real`, keeping Korean out.
function passwordEdited() {
  const input = $("password");
  const next = [...input.value];
  const old = pw.shown;
  let start = 0;
  while (start < old.length && start < next.length && old[start] === next[start]) start++;
  let end = 0;
  while (end < old.length - start && end < next.length - start && old[old.length - 1 - end] === next[next.length - 1 - end]) end++;
  const typed = next.slice(start, next.length - end);
  const kept = typed.filter((c) => !HANGUL.test(c));
  if (kept.length < typed.length) pw.refused = true;
  else if (kept.length) pw.refused = false;
  pw.real = [...pw.real.slice(0, start), ...kept, ...pw.real.slice(old.length - end)];
  clearTimeout(pw.timer);
  pw.peek = kept.length === 1 && !pw.revealed ? start : -1;
  if (pw.peek >= 0) {
    pw.timer = setTimeout(() => {
      const caret = caretChars();
      pw.peek = -1;
      showPassword(caret);
    }, 1000);
  }
  showPassword(start + kept.length);
}

/// Where the caret is, in characters.
function caretChars() {
  const input = $("password");
  return [...input.value.slice(0, input.selectionStart || 0)].length;
}

function passwordHint(event) {
  const input = $("password");
  if (event && typeof event.getModifierState === "function") pw.capsLock = event.getModifierState("CapsLock");
  const focused = document.activeElement === input;
  if (!focused) pw.refused = false;
  const text = !focused ? "" : pw.refused ? T.hangulInput : pw.capsLock ? T.capsLock : "";
  $("login-hint").textContent = text;
  $("login-hint").hidden = !text;
}

function clearPassword() {
  clearTimeout(pw.timer);
  pw.real = [];
  pw.peek = -1;
  showPassword(null);
}

function setupPasswordBox() {
  const input = $("password");
  // A Korean syllable being composed is read once it is done (then refused).
  input.addEventListener("input", (event) => {
    if (!event.isComposing) passwordEdited();
    passwordHint(event);
  });
  input.addEventListener("compositionend", (event) => {
    passwordEdited();
    passwordHint(event);
  });
  for (const type of ["keydown", "keyup", "focus", "blur"]) input.addEventListener(type, passwordHint);
  const eye = $("password-eye");
  eye.addEventListener("pointerdown", (event) => event.preventDefault()); // the box keeps the keyboard
  eye.addEventListener("click", () => {
    pw.revealed = !pw.revealed;
    pw.peek = -1;
    eye.setAttribute("aria-pressed", String(pw.revealed));
    eye.setAttribute("aria-label", pw.revealed ? T.hidePassword : T.showPassword);
    showPassword(document.activeElement === input ? caretChars() : null);
  });
  eye.setAttribute("aria-label", T.showPassword);
}

async function signOut() {
  try { await api("/api/logout", {}); } catch (_) { /* signed out either way */ }
  stopEvents();
  start();
}

function showLogin(locked, me) {
  stopEvents();
  $("app").hidden = true;
  $("login").hidden = false;
  $("password").placeholder = T.password;
  $("login-button").textContent = T.signIn;
  $("link-privacy").textContent = T.privacy;
  $("link-terms").textContent = T.terms;
  $("link-eula").textContent = T.eula;
  if (me) renderConnection(me);
  if (locked) lockedFor(locked);
}

/// This device and where it connects to, as Tailscale and the browser know them. Only shown.
function renderConnection(me) {
  const list = $("connection");
  list.replaceChildren();
  const row = (label, value) => {
    if (!value) return;
    list.append(el("dt", null, label), el("dd", null, value));
  };
  const d = me.device || {};
  row(T.device, [d.name, d.os].filter(Boolean).join(" · "));
  row(T.target, me.target);
  if (location.protocol === "https:") row(T.secure, T.secureValue);
  row(T.browser, browserName());
  list.append(el("dd", "note", T.shownOnly));
  list.hidden = list.childElementCount <= 1;
}

function browserName() {
  const ua = navigator.userAgent;
  const browser = /Edg\//.test(ua) ? "Edge" : /Firefox\/|FxiOS/.test(ua) ? "Firefox" : /Chrome\/|CriOS/.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "";
  const system = /iPhone/.test(ua) ? "iPhone" : /iPad/.test(ua) ? "iPad" : /Android/.test(ua) ? "Android" : /Mac OS X/.test(ua) ? "macOS" : /Windows/.test(ua) ? "Windows" : /Linux/.test(ua) ? "Linux" : "";
  return [browser, system].filter(Boolean).join(" · ");
}

// ---- Live updates ----

function stopEvents() {
  if (state.events) state.events.close();
  state.events = null;
}

/// One stream: the overview always, and the screen of the pane being looked at.
function listen() {
  stopEvents();
  const source = new EventSource(state.pane == null ? "/api/events" : "/api/events?pane=" + encodeURIComponent(state.pane));
  state.events = source;
  source.addEventListener("sessions", (e) => onOverview(JSON.parse(e.data)));
  source.addEventListener("screen", (e) => onScreen(JSON.parse(e.data)));
  source.addEventListener("signedout", () => showLogin());
  source.onopen = () => {
    setLive(true);
    state.sized = null;
    fitTerm();
  };
  source.onerror = async () => {
    setLive(false);
    // A stream also ends when the session did; ask which before reconnecting.
    try {
      const { data } = await api("/api/me");
      if (!data.signedIn) showLogin(data.lockedFor, data);
    } catch (_) { /* offline: EventSource retries by itself */ }
  };
}

function setLive(on) {
  const dot = $("live");
  dot.classList.toggle("on", on);
  dot.title = on ? T.live : T.offline;
}

// ---- What there is ----

function onOverview(data) {
  const sessions = data.sessions || [];
  notifyChanges(sessions);
  state.workspaces = data.workspaces || [];
  state.sessions = sessions;
  state.plugins = data.plugins || [];
  state.voice = !!data.voice;
  // If a recording is running and its terminal just closed, or voice was turned off, stop the mic
  // — its button is about to be hidden, which would otherwise leave no way to stop it.
  if (recorder && (!state.voice || !sessions.some((s) => s.pane === state.pane))) stopRecording();
  if (!state.started) {
    state.started = true;
    return pickStart();
  }
  // A tab asked for here has opened: show it. It is the workspace's last tab (not always its
  // active one: the Mac keeps the tab it shows).
  const pending = state.pendingNew && workspace(state.pendingNew.workspace);
  if (pending && pending.tabs.length > state.pendingNew.count) {
    state.pendingNew = null;
    state.workspace = pending.id;
    return selectTab(pending.tabs.length - 1);
  }
  // A workspace opened from the folder sheet is here: show it (or the tab it got, when its
  // folder was open already).
  if (state.pendingFolder) {
    const { ids, tabs } = state.pendingFolder;
    const fresh = state.workspaces.find((w) => !ids.includes(w.id));
    const grown = state.workspaces.find((w) => tabs[w.id] != null && w.tabs.length > tabs[w.id]);
    if (fresh || grown) {
      state.pendingFolder = null;
      state.workspace = (fresh || grown).id;
      return selectTab((fresh || grown).tabs.length - 1);
    }
  }
  // A workspace woken up here has its terminals now: show it if it is still the one on screen.
  const woken = state.waking != null && workspace(state.waking);
  if (woken && !woken.sleeping) {
    state.waking = null;
    if (state.workspace === woken.id && state.pane == null) return selectWorkspace(woken.id);
  }
  if (state.workspace != null && !workspace(state.workspace)) return goHome();
  // The pane being looked at may have closed: fall back to its workspace, then home.
  if (state.pane != null && !session(state.pane)) {
    state.pane = null;
    const ws = workspace(state.workspace);
    if (ws && !ws.sleeping) return selectWorkspace(ws.id);
    return goHome();
  }
  render();
}

/// Where to start: the pane in the address (a notification's link), else home.
function pickStart() {
  if (!openAskedPane()) goHome();
}

/// Opens the pane the address names (`#6`), if it is still there: an agent's session, or any
/// terminal of a tab. Whether it did.
function openAskedPane() {
  const asked = Number((location.hash || "").slice(1));
  if (!asked || !Number.isInteger(asked)) return false;
  if (session(asked)) {
    selectPane(asked);
    return true;
  }
  for (const ws of state.workspaces) {
    const index = ws.tabs.findIndex((tab) => tab.panes.includes(asked));
    if (index >= 0 && !ws.sleeping) {
      state.workspace = ws.id;
      state.tab = index;
      switchPane(asked);
      return true;
    }
  }
  return false;
}

function goHome() {
  state.workspace = null;
  state.tab = 0;
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);
  switchPane(null);
}

function selectWorkspace(id) {
  const ws = workspace(id);
  if (!ws) return;
  state.workspace = id;
  const tab = ws.tabs[ws.active_tab] || ws.tabs[0];
  if (!tab) {
    state.tab = 0;
    switchPane(null);
    return;
  }
  selectTab(ws.tabs.indexOf(tab));
}

function selectTab(index) {
  const ws = workspace(state.workspace);
  const tab = ws && ws.tabs[index];
  if (!tab) return;
  state.tab = index;
  switchPane(tab.panes.includes(tab.active_pane) ? tab.active_pane : tab.panes[0]);
}

function selectPane(pane) {
  const s = session(pane);
  if (!s) return;
  state.workspace = s.workspace_id;
  state.tab = s.tab;
  switchPane(pane);
}

function switchPane(pane) {
  const changed = pane !== state.pane;
  if (changed) stopRecording(); // don't let a recording carry over to another terminal
  state.pane = pane;
  if (changed) {
    state.rows = [];
    state.lines = [];
    state.screen = null;
    state.follow = true;
    $("term").replaceChildren();
    if (pane != null) history.replaceState(null, "", "#" + pane);
    listen();
  }
  closeDrawers();
  render();
}

// ---- Drawing ----

// Each part is drawn again only when what it shows changed. The overview arrives several times
// a second while someone types (every keystroke moves a session's last activity), and drawing
// the sidebar, the tabs and the key bar anew each time made the page flicker.
const drawn = {};
function redraw(part, signature, draw) {
  const key = JSON.stringify(signature);
  if (drawn[part] === key) return;
  drawn[part] = key;
  draw();
}

function render() {
  const ws = workspace(state.workspace);
  const tab = ws && ws.tabs[state.tab];
  const shown = (s) => s && [s.pane, s.workspace_id, s.tab, s.title, s.tool, s.status, s.color, s.needs_user, s.working, s.unread, s.asks];
  const marks = state.sessions.map((s) => [s.pane, s.workspace_id, s.needs_user, s.working]);
  redraw("badge", [marks, state.workspace], renderBadge);
  redraw("workspaces", [state.workspaces, marks, state.workspace], renderWorkspaces);
  redraw("header", [ws, shown(session(state.pane)), state.tab], renderHeader);
  redraw("tabs", [ws && ws.tabs, state.tab, marks], renderTabs);
  redraw("panes", [tab, tab && tab.panes.map((p) => [shown(session(p)), session(p)?.elapsed]), state.pane], renderPanes);
  redraw("body", [state.workspace, state.workspaces.length, ws && ws.sleeping, state.waking, shown(session(state.pane)), state.ctrl, state.size, inChat(), !!currentChat(), state.voice], renderBody);
  redraw("chat", [inChat() && currentChat()], renderChat);
  if (state.workspace == null) {
    const busy = state.sessions.map((s) => [shown(s), s.working ? s.elapsed : null]);
    redraw("home", [state.workspaces, busy, state.plugins, state.waking], renderHome);
  } else {
    drawn.home = null;
  }
  $("home-link").classList.toggle("active", state.workspace == null);
  const waiting = state.sessions.filter((s) => s.needs_user).length;
  document.title = waiting ? `(${waiting}) Agentty` : "Agentty";
}

function renderBadge() {
  // Waiting outside the workspace on screen: the menu says so.
  const elsewhere = state.sessions.filter((s) => s.needs_user && s.workspace_id !== state.workspace).length;
  const badge = $("menu-badge");
  badge.hidden = elsewhere === 0;
  badge.textContent = elsewhere;
}

function newWorkspaceButton() {
  const b = el("button", "new-workspace", "+ " + T.newWorkspace);
  b.type = "button";
  b.addEventListener("click", () => openFolderSheet(""));
  return b;
}

function renderWorkspaces() {
  const root = $("workspaces");
  root.replaceChildren();
  if (!state.workspaces.length) {
    root.append(el("div", "empty-note", T.noWorkspaces), newWorkspaceButton());
    return;
  }
  let lastGroup;
  for (const ws of state.workspaces) {
    if (ws.group != null && ws.group !== lastGroup) {
      const head = el("div", "group-head");
      const swatch = el("span", "swatch");
      const color = safeColor(ws.group_color);
      swatch.style.background = color || "var(--border)";
      head.append(swatch, el("span", null, ws.group));
      root.append(head);
    }
    lastGroup = ws.group;
    root.append(workspaceCard(ws));
  }
  root.append(newWorkspaceButton());
}

function workspaceCard(ws) {
  const color = safeColor(ws.color);
  const card = el("button", "ws-card" + (color ? " colored" : "") + (ws.id === state.workspace ? " active" : "") + (ws.sleeping ? " sleeping" : ""));
  card.type = "button";
  if (color) card.style.background = color;
  card.append(el("div", "name", ws.name));
  const sub = [ws.branch, ws.folder].filter(Boolean).join(" · ");
  if (sub) card.append(el("div", "sub", sub));
  const mine = state.sessions.filter((s) => s.workspace_id === ws.id);
  const needs = mine.filter((s) => s.needs_user).length;
  const marks = el("div", "marks");
  if (mine.some((s) => s.working)) marks.append(el("span", "working"));
  if (needs) marks.append(el("span", "needs", needs));
  card.append(marks);
  card.addEventListener("click", () => selectWorkspace(ws.id));
  return card;
}

function renderHeader() {
  const ws = workspace(state.workspace);
  $("ws-name").textContent = ws ? ws.name : T.home;
  const dot = $("ws-dot");
  dot.hidden = !ws;
  dot.style.background = (ws && safeColor(ws.color)) || "var(--muted)";
  $("ws-sub").textContent = ws ? [ws.branch, ws.folder].filter(Boolean).join(" · ") : "";
  const s = session(state.pane);
  const chip = $("pane-status");
  chip.hidden = !s;
  if (s) {
    chip.textContent = s.status || "";
    chip.style.color = safeColor(s.color) || "var(--muted)";
  }
  const tab = ws && ws.tabs[state.tab];
  const split = tab ? tab.panes.length : 0;
  $("panes-button").hidden = split < 2;
  $("panes-count").textContent = split;
}

function renderTabs() {
  const nav = $("tabs");
  nav.replaceChildren();
  const ws = workspace(state.workspace);
  nav.hidden = !ws;
  if (!ws) return;
  ws.tabs.forEach((tab, index) => {
    const b = el("button", "tab" + (index === state.tab ? " active" : ""));
    b.type = "button";
    const panes = tab.panes.map(session).filter(Boolean);
    const mark = el("span", "state");
    if (panes.some((s) => s.needs_user)) mark.classList.add("needs");
    else if (panes.some((s) => s.working)) mark.classList.add("working");
    b.append(mark, el("span", "label", tab.title));
    if (tab.panes.length > 1) b.append(el("span", "split", "▥" + tab.panes.length));
    b.addEventListener("click", () => selectTab(index));
    nav.append(b);
    if (index === state.tab) requestAnimationFrame(() => b.scrollIntoView({ block: "nearest", inline: "nearest" }));
  });
  const add = el("button", "tab new-tab", "+");
  add.type = "button";
  add.title = T.newTab;
  add.setAttribute("aria-label", T.newTab);
  add.addEventListener("click", () => openNewSheet(ws.id));
  nav.append(add);
}

const TOOL_MARK = { claude: "✳", codex: "◎", gemini: "✦", shell: "$" };

function renderPanes() {
  $("panes-title").textContent = T.panes;
  const list = $("pane-list");
  list.replaceChildren();
  const ws = workspace(state.workspace);
  const tab = ws && ws.tabs[state.tab];
  if (!tab) return;
  tab.panes.forEach((pane, number) => {
    const s = session(pane);
    if (!s) return;
    const card = el("button", "pane-card" + (pane === state.pane ? " active" : "") + (s.needs_user ? " needs" : ""));
    card.type = "button";
    const body = el("div", "body");
    body.append(el("div", "name", `${number + 1}. ${s.title}`));
    const line = el("div", "line");
    const chip = el("span", "chip", s.status || "");
    chip.style.color = safeColor(s.color) || "var(--muted)";
    line.append(chip);
    if (s.working && s.elapsed != null) line.append(el("span", "muted", elapsed(s.elapsed)));
    body.append(line);
    if (s.asks) body.append(el("div", "asks-text", s.asks));
    card.append(el("div", "tool", TOOL_MARK[s.tool] || "•"), body);
    card.addEventListener("click", () => switchPane(pane));
    list.append(card);
  });
}

function elapsed(seconds) {
  const m = Math.floor(seconds / 60);
  return m ? `${m}m ${seconds % 60}s` : `${seconds}s`;
}

/// The middle: a terminal, or why there is none.
function renderBody() {
  const home = state.workspace == null;
  const ws = workspace(state.workspace);
  const s = session(state.pane);
  $("home").hidden = !home;
  const note = home ? null : ws && ws.sleeping ? T.sleeping : null;
  const placeholder = $("placeholder");
  placeholder.hidden = !note;
  placeholder.replaceChildren();
  if (note) {
    placeholder.append(el("p", null, note));
    const actions = el("div", "actions");
    actions.append(wakeButton(ws, "primary"));
    placeholder.append(actions);
  }
  const chatting = inChat();
  for (const id of ["term-wrap", "keys"]) $(id).hidden = home || !!note || !s || chatting;
  $("prompt-form").hidden = home || !!note || !s;
  $("chat").hidden = !chatting;
  $("prompt").placeholder = chatting ? T.chatHint : T.promptHint;
  // The lead of a chat: one switch between its chat and its terminal.
  const chat = currentChat();
  const toggle = $("chat-toggle");
  toggle.hidden = !(chat && state.pane === chat.lead && s && !s.needs_user);
  toggle.textContent = chatting ? T.terminal : T.chat;
  if (!chatting && !home) requestAnimationFrame(fitTerm);
  // The mic sits in the prompt bar, but only when the Mac offers voice-to-text.
  $("mic-button").hidden = home || !!note || !s || !state.voice;
  $("asks").hidden = !(s && s.asks);
  $("asks").textContent = (s && s.asks) || "";
  renderKeys(s && s.needs_user);
}

/// The lead of a chat tab, as the chat: what the team runs on, then the conversation.
function renderChat() {
  const chat = inChat() && currentChat();
  const head = $("chat-head");
  const log = $("chat-log");
  if (!chat) {
    head.replaceChildren();
    log.replaceChildren();
    return;
  }
  // At the bottom (or close), the log follows what comes in; scrolled up, it stays put.
  const following = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
  head.replaceChildren();
  const line = el("div", "chat-meta");
  if (chat.branch) line.append(el("span", "chat-branch", "⎇ " + chat.branch));
  for (const role of chat.roles || []) line.append(el("span", "muted", role));
  for (const u of chat.usage || []) {
    const chip = el("span", "chip usage level-" + (Number(u.level) || 0), u.label);
    chip.title = u.resets || "";
    line.append(chip);
  }
  head.append(line);
  if (chat.warning) head.append(el("div", "chat-warning", chat.warning));
  log.replaceChildren();
  const entries = chat.entries || [];
  if (!entries.length) log.append(el("div", "chat-empty", T.chatEmpty));
  for (const entry of entries) log.append(chatEntry(entry));
  if (following) log.scrollTop = log.scrollHeight;
}

/// One entry: the user's bubble, the lead's reply (tool steps folded to a few muted lines), or a
/// worker's report.
function chatEntry(entry) {
  const role = ["user", "lead", "report"].includes(entry.role) ? entry.role : "lead";
  const box = el("div", "msg " + role);
  if (role !== "lead") {
    box.append(el("div", "text", entry.text || ""));
    return box;
  }
  let prose = [];
  let steps = [];
  const flushProse = () => {
    const text = prose.join("\n").replace(/\*\*/g, "").trim();
    if (text) box.append(el("div", "text", text));
    prose = [];
  };
  const flushSteps = () => {
    steps.slice(0, 3).forEach((step) => box.append(el("div", "step", step)));
    if (steps.length > 3) box.append(el("div", "step", "+" + (steps.length - 3)));
    steps = [];
  };
  for (const line of (entry.text || "").split("\n")) {
    const tool = line.match(/^\[tool: (.*)\]$/);
    if (tool) {
      flushProse();
      steps.push(tool[1]);
    } else {
      flushSteps();
      prose.push(line);
    }
  }
  flushSteps();
  flushProse();
  return box;
}

function renderKeys(needs) {
  const keys = $("keys");
  keys.replaceChildren();
  const add = (label, key, extra = {}, cls) => {
    const b = el("button", cls, label);
    b.type = "button";
    b.addEventListener("click", () => {
      if (key === "ctrl") {
        state.ctrl = !state.ctrl;
        b.classList.toggle("on", state.ctrl);
        return;
      }
      sendKey(key, extra);
    });
    keys.append(b);
    return b;
  };
  // Enter first and larger: the key pressed most, within reach whatever the row is scrolled to.
  add("⏎", "enter", {}, "enter");
  if (needs) {
    add("1", "1", {}, "answer");
    add("2", "2", {}, "answer");
    add("3", "3", {}, "answer");
  }
  add("Esc", "escape");
  add("Tab", "tab");
  add("⇧Tab", "tab", { shift: true });
  add("↑", "up");
  add("↓", "down");
  add("←", "left");
  add("→", "right");
  add("^C", "c", { ctrl: true });
  add("Ctrl", "ctrl").classList.toggle("on", state.ctrl);
  // Text size, kept on this device.
  for (const [label, step] of [["A−", -1], ["A+", 1]]) {
    const b = el("button", null, label);
    b.type = "button";
    b.addEventListener("click", () => {
      state.size = Math.max(MIN_SIZE, Math.min(MAX_SIZE, state.size + step));
      try { localStorage.setItem("agentty.size", String(state.size)); } catch (_) { /* private mode */ }
      layoutTerm();
      fitTerm();
    });
    keys.append(b);
  }
}

// ---- Home ----

/// Everything at a glance: what waits for the user, what works, the workspaces (woken or given a
/// tab from here) and the plugins with their automations.
function renderHome() {
  const root = $("home");
  root.replaceChildren();
  const needs = state.sessions.filter((s) => s.needs_user);
  const working = state.sessions.filter((s) => s.working && !s.needs_user);
  const asleep = state.workspaces.filter((w) => w.sleeping).length;
  const running = state.plugins.filter((p) => p.state === "running").length;

  const tiles = el("div", "tiles");
  const tile = (label, value, sub, tone) => {
    const t = el("div", "tile" + (tone ? " " + tone : ""));
    t.append(el("div", "tile-label", label), el("div", "tile-value", value));
    if (sub) t.append(el("div", "tile-sub", sub));
    tiles.append(t);
  };
  tile(T.needsYou, needs.length, null, needs.length ? "attention" : null);
  tile(T.workingNow, working.length, null, working.length ? "accent" : null);
  tile(T.workspacesTitle, state.workspaces.length, asleep ? `${T.asleep} ${asleep}` : null);
  tile(T.plugins, state.plugins.length, state.plugins.length ? T.running.replace("{n}", running) : null);
  root.append(tiles);

  const section = (title, count) => {
    const box = el("section", "home-section");
    const head = el("h2", null, title);
    if (count != null) head.append(el("span", "count", count));
    box.append(head);
    root.append(box);
    return box;
  };

  const sessionRow = (s, extra) => {
    const row = el("button", "home-row" + (s.needs_user ? " needs" : ""));
    row.type = "button";
    row.append(el("div", "tool", TOOL_MARK[s.tool] || "•"));
    const body = el("div", "body");
    body.append(el("div", "name", s.title || s.workspace));
    const line = el("div", "line");
    const chip = el("span", "chip", s.status || "");
    chip.style.color = safeColor(s.color) || "var(--muted)";
    line.append(chip, el("span", "muted", s.workspace));
    if (extra) line.append(el("span", "muted", extra));
    body.append(line);
    if (s.asks) body.append(el("div", "asks-text", s.asks));
    row.append(body);
    row.addEventListener("click", () => selectPane(s.pane));
    return row;
  };

  const waitingBox = section(T.needsYou, needs.length || null);
  if (!needs.length) waitingBox.append(el("div", "empty-note", T.nothingWaiting));
  for (const s of needs) waitingBox.append(sessionRow(s));

  const workingBox = section(T.workingNow, working.length || null);
  if (!working.length) workingBox.append(el("div", "empty-note", T.nothingWorking));
  for (const s of working) workingBox.append(sessionRow(s, s.elapsed != null ? elapsed(s.elapsed) : null));

  const wsBox = section(T.workspacesTitle, state.workspaces.length || null);
  if (!state.workspaces.length) wsBox.append(el("div", "empty-note", T.noWorkspaces));
  for (const ws of state.workspaces) {
    const row = el("div", "home-ws" + (ws.sleeping ? " sleeping" : ""));
    const open = el("button", "home-ws-open");
    open.type = "button";
    const dot = el("span", "ws-dot");
    dot.style.background = safeColor(ws.color) || "var(--muted)";
    const text = el("div", "body");
    const name = el("div", "name");
    name.append(dot, el("span", null, ws.name));
    text.append(name);
    const mine = state.sessions.filter((s) => s.workspace_id === ws.id);
    const sub = ws.sleeping ? T.asleep : T.open.replace("{n}", mine.length);
    text.append(el("div", "sub", [sub, ws.branch, ws.folder].filter(Boolean).join(" · ")));
    open.append(text);
    const marks = el("div", "marks");
    const waiting = mine.filter((s) => s.needs_user).length;
    if (mine.some((s) => s.working)) marks.append(el("span", "working"));
    if (waiting) marks.append(el("span", "needs", waiting));
    open.append(marks);
    open.addEventListener("click", () => selectWorkspace(ws.id));
    row.append(open);
    const actions = el("div", "actions");
    if (ws.sleeping) actions.append(wakeButton(ws));
    const add = el("button", "icon-text", "+");
    add.type = "button";
    add.title = T.newTab;
    add.setAttribute("aria-label", T.newTab);
    add.addEventListener("click", () => openNewSheet(ws.id));
    actions.append(add);
    row.append(actions);
    wsBox.append(row);
  }
  wsBox.append(newWorkspaceButton());

  const pluginBox = section(T.plugins, state.plugins.length || null);
  if (!state.plugins.length) pluginBox.append(el("div", "empty-note", T.noPlugins));
  for (const p of state.plugins) pluginBox.append(pluginCard(p));
}

const PLUGIN_TONE = { running: "success", starting: "accent", failed: "error", consent: "attention" };

function pluginCard(p) {
  const card = el("div", "plugin-card" + (p.state === "off" ? " off" : ""));
  const head = el("div", "plugin-head");
  const name = el("div", "name", p.name);
  if (p.version) name.append(el("span", "muted version", "v" + p.version));
  const chip = el("span", "chip " + (PLUGIN_TONE[p.state] || "muted"), T.pstate[p.state] || p.state);
  head.append(name, chip);
  card.append(head);
  if (p.error) card.append(el("div", "plugin-error", p.error));
  for (const a of p.automations || []) {
    const line = el("div", "automation" + (a.sleeping ? " sleeping" : ""));
    line.append(el("span", "state " + (a.sleeping ? "" : a.state)));
    // Untitled: the plugin's own status, not one of its tabs.
    if (a.title) line.append(el("span", "title", a.title));
    const what = a.sleeping ? T.asleep : [T.astate[a.state], a.text].filter(Boolean).join(" · ");
    if (what) line.append(el("span", "muted what", what));
    if (a.state === "working" && a.elapsed != null && !a.sleeping) line.append(el("span", "muted", elapsed(a.elapsed)));
    card.append(line);
  }
  return card;
}

// ---- Waking workspaces, new tabs ----

function wakeButton(ws, cls) {
  const b = el("button", cls || null, state.waking === ws.id ? T.waking : T.wake);
  b.type = "button";
  b.disabled = state.waking === ws.id;
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    wakeWorkspace(ws.id);
  });
  return b;
}

async function workspaceAction(body) {
  try {
    const { status } = await api("/api/workspace", body);
    if (status === 401) showLogin();
    else if (status === 429) alert(body.action === "new" ? T.tooManyTabs : T.busy);
    return status === 200;
  } catch (_) {
    return false; // offline; the dot says so
  }
}

async function wakeWorkspace(id) {
  state.waking = id;
  render();
  if (!(await workspaceAction({ workspace: id, action: "wake" }))) {
    state.waking = null;
    render();
  }
}

function openNewSheet(id) {
  const ws = workspace(id);
  if (!ws) return;
  closeDrawers();
  state.sheetFor = id;
  $("new-title").textContent = T.newIn.replace("{ws}", ws.name);
  const choices = $("new-choices");
  choices.replaceChildren();
  for (const [tool, label] of [["shell", T.terminal], ["claude", T.claude], ["codex", T.codex]]) {
    const b = el("button", "sheet-choice");
    b.type = "button";
    b.append(el("span", "tool", TOOL_MARK[tool] || "•"), el("span", null, label));
    b.addEventListener("click", () => newTab(id, tool));
    choices.append(b);
  }
  $("new-cancel").textContent = T.cancel;
  $("new-sheet").hidden = false;
  $("scrim").hidden = false;
}

function closeSheet() {
  state.sheetFor = null;
  $("new-sheet").hidden = true;
  $("scrim").hidden = true;
}

async function newTab(id, tool) {
  const ws = workspace(id);
  closeSheet();
  if (!ws) return;
  // A sleeping workspace's saved tabs come back too: the new one is past them, not just past
  // what shows now. Counting what shows is enough: any growth means the app took the request.
  state.pendingNew = { workspace: id, count: ws.sleeping ? 0 : ws.tabs.length };
  if (!(await workspaceAction({ workspace: id, action: "new", tool }))) state.pendingNew = null;
}

// ---- New workspaces, in folders under the remote folder on the Mac ----

const joinPath = (path, name) => (path ? path + "/" + name : name);

async function openFolderSheet(path) {
  closeDrawers();
  closeSheet();
  let answer;
  try {
    answer = await api("/api/folders?path=" + encodeURIComponent(path));
  } catch (_) {
    return; // offline; the dot says so
  }
  if (answer.status === 401) return showLogin();
  // Gone since (or never there): back to the top.
  if (answer.status === 404 && path) return openFolderSheet("");
  if (answer.status !== 200) return alert(T.failed);
  state.folderSheet = { path, root: answer.data.root, folders: answer.data.folders || [] };
  renderFolderSheet();
  $("folder-sheet").hidden = false;
  $("scrim").hidden = false;
}

function renderFolderSheet() {
  const sheet = state.folderSheet;
  if (!sheet) return;
  $("folder-title").textContent = T.newWorkspace;
  const crumbs = $("folder-path");
  crumbs.replaceChildren();
  const names = sheet.path ? sheet.path.split("/") : [];
  [sheet.root, ...names].forEach((name, i) => {
    if (i) crumbs.append(el("span", "sep", "/"));
    const b = el("button", "crumb", name);
    b.type = "button";
    b.disabled = i === names.length;
    b.addEventListener("click", () => openFolderSheet(names.slice(0, i).join("/")));
    crumbs.append(b);
  });
  const list = $("folder-list");
  list.replaceChildren();
  if (sheet.path) {
    const up = el("button", "folder-row up", "↑ " + T.up);
    up.type = "button";
    up.addEventListener("click", () => openFolderSheet(names.slice(0, -1).join("/")));
    list.append(up);
  }
  if (!sheet.folders.length) list.append(el("div", "empty-note", T.noFolders));
  for (const name of sheet.folders) {
    const b = el("button", "folder-row");
    b.type = "button";
    b.append(el("span", "folder-icon"), el("span", "folder-name", name), el("span", "chevron", "›"));
    b.addEventListener("click", () => openFolderSheet(joinPath(sheet.path, name)));
    list.append(b);
  }
  $("folder-new-name").placeholder = T.folderName;
  $("folder-new-name").setAttribute("aria-label", T.newFolder);
  $("folder-create").textContent = T.create;
  $("folder-error").textContent = "";
  $("folder-open-label").textContent = T.openHere;
  const choices = $("folder-choices");
  choices.replaceChildren();
  for (const [tool, label] of [["shell", T.terminal], ["claude", T.claude], ["codex", T.codex]]) {
    const b = el("button", "sheet-choice");
    b.type = "button";
    b.append(el("span", "tool", TOOL_MARK[tool] || "•"), el("span", null, label));
    b.addEventListener("click", () => openFolder(sheet.path, tool));
    choices.append(b);
  }
  $("folder-hint").textContent = T.folderHint.replace("{root}", "~/" + sheet.root);
  $("folder-cancel").textContent = T.cancel;
}

function closeFolderSheet() {
  state.folderSheet = null;
  $("folder-sheet").hidden = true;
  $("scrim").hidden = true;
}

async function createFolder() {
  const sheet = state.folderSheet;
  const input = $("folder-new-name");
  const name = input.value.trim();
  if (!sheet || !name) return input.focus();
  const error = $("folder-error");
  if (name.startsWith(".") || /[\/\\:*?"<>|]/.test(name)) {
    error.textContent = T.badFolderName;
    return;
  }
  let answer;
  try {
    answer = await api("/api/workspace", { action: "mkdir", path: sheet.path, name });
  } catch (_) {
    return;
  }
  if (answer.status === 401) return showLogin();
  if (answer.status === 409) error.textContent = T.folderExists;
  else if (answer.status === 400) error.textContent = T.badFolderName;
  else if (answer.status === 429) error.textContent = T.tooManyTabs;
  else if (answer.status !== 200) error.textContent = T.failed;
  else {
    input.value = "";
    openFolderSheet(joinPath(sheet.path, name));
  }
}

async function openFolder(path, tool) {
  closeFolderSheet();
  const tabs = {};
  for (const w of state.workspaces) tabs[w.id] = w.sleeping ? 0 : w.tabs.length;
  state.pendingFolder = { ids: state.workspaces.map((w) => w.id), tabs };
  try {
    const { status } = await api("/api/workspace", { action: "open", path, tool });
    if (status === 401) showLogin();
    else if (status === 429) alert(T.tooManyTabs);
    else if (status !== 200) alert(T.failed);
    if (status !== 200) state.pendingFolder = null;
  } catch (_) {
    state.pendingFolder = null;
  }
}

// ---- Drawers ----

function openDrawer(id) {
  closeDrawers();
  $(id).classList.add("open");
  $("scrim").hidden = false;
}

function closeDrawers() {
  for (const id of ["sidebar", "panes"]) $(id).classList.remove("open");
  $("scrim").hidden = true;
}

// ---- Input ----

async function input(body) {
  try {
    const { status } = await api("/api/input", body);
    if (status === 401) showLogin();
    else if (status === 413) alert(T.tooLong);
    else if (status === 429) alert(T.busy);
  } catch (_) { /* offline; the dot says so */ }
}

function sendKey(key, extra = {}) {
  if (state.pane == null) return;
  const ctrl = !!extra.ctrl || state.ctrl;
  if (state.ctrl) {
    state.ctrl = false;
    renderKeys(session(state.pane)?.needs_user);
  }
  input({ pane: state.pane, kind: "key", key, ctrl, alt: !!extra.alt, shift: !!extra.shift });
}

function sendText(text) {
  if (state.pane == null || !text) return;
  if (state.ctrl && text.length === 1 && /[a-z]/i.test(text)) return sendKey(text.toLowerCase(), { ctrl: true });
  input({ pane: state.pane, kind: "text", text });
}

// ---- The terminal ----

// The cursor as [column, line], the line counted from the top of the scrollback sent above the
// screen.
function cursorLine(screen) {
  return screen && screen.cursor ? [screen.cursor[0], (screen.history || 0) + screen.cursor[1]] : null;
}

// Lines of scrollback come above the screen. At the bottom (or close), the terminal follows what
// comes in; scrolled up, it stays on the lines being read while new ones arrive below, and a
// button leads back down.
function onScreen(update) {
  if (update.pane !== state.pane) return;
  const term = $("term");
  const wrap = $("term-wrap");
  // The first line in view and where it sits, to keep it there when lines leave the top.
  let anchor = null;
  if (!state.follow) {
    const top = wrap.scrollTop;
    const row = state.rows.find((r) => !r.hidden && r.offsetTop + r.offsetHeight > top);
    if (row) anchor = { row, offset: row.offsetTop - top };
  }
  let before = cursorLine(state.screen);
  if (update.full || !state.screen || state.screen.cols !== update.cols) {
    term.replaceChildren();
    state.rows = [];
    state.lines = [];
    before = null;
  } else if (update.shift) {
    // Lines that left the top of the kept scrollback.
    for (const row of state.rows.splice(0, update.shift)) row.remove();
    state.lines.splice(0, update.shift);
    if (before) before[1] -= update.shift;
  }
  const len = update.len != null ? update.len : update.rows;
  while (state.rows.length > len) {
    state.rows.pop().remove();
    state.lines.pop();
  }
  while (state.rows.length < len) {
    const row = el("div", "row");
    term.append(row);
    state.rows.push(row);
    state.lines.push([]);
  }
  state.screen = update;
  term.style.background = hex(update.bg);
  term.style.color = hex(update.fg);
  // The frame around it too, so a scrollbar's gutter shows no stripe of another color.
  wrap.style.background = hex(update.bg);
  const dirty = new Set();
  for (const [index, runs] of update.lines) {
    if (index >= state.rows.length) continue;
    state.lines[index] = runs;
    dirty.add(index);
  }
  // The cursor is drawn inside its row's text, so a row it left or entered is drawn again too.
  const cursor = cursorLine(update);
  if (before && before[1] >= 0) dirty.add(before[1]);
  if (cursor) dirty.add(cursor[1]);
  for (const index of dirty) renderRow(index);
  // Empty rows under the last text (or the cursor) are left out: a shell's prompt sits at the
  // top of a tall screen, and scrolling to the bottom would show nothing but blank rows.
  let last = cursor ? cursor[1] : 0;
  state.lines.forEach((runs, index) => {
    if (runs.some((r) => r.t.trim() || r.b != null)) last = Math.max(last, index);
  });
  state.rows.forEach((row, index) => (row.hidden = index > last));
  layoutTerm();
  if (state.follow) {
    wrap.scrollTop = wrap.scrollHeight;
  } else if (anchor && anchor.row.isConnected && !anchor.row.hidden) {
    wrap.scrollTop = anchor.row.offsetTop - anchor.offset;
  }
  showToLatest();
}

// Following again once the reader is back at the bottom; the button shows while they are not.
function onTermScroll() {
  const wrap = $("term-wrap");
  state.follow = wrap.scrollHeight - wrap.scrollTop - wrap.clientHeight < 40;
  showToLatest();
}

function showToLatest() {
  const button = $("to-latest");
  if (button.hidden !== state.follow) button.hidden = state.follow;
}

// A program that scrolls itself (Claude Code's full-screen UI, vim, less) keeps no scrollback
// in the terminal: there is nothing here to scroll, so a swipe or the wheel goes to it as the
// mouse wheel would, a line per line's height moved.
let pendingScroll = 0;
let scrollTimer = null;

function programScrolls() {
  return !!(state.screen && state.screen.programScrolls);
}

function scrollProgram(pixels) {
  pendingScroll += pixels / (1.25 * state.size);
  if (scrollTimer) return;
  // Gathered for a moment, so a swipe is a few requests, not one per touch event.
  scrollTimer = setTimeout(() => {
    scrollTimer = null;
    const lines = Math.max(-60, Math.min(60, Math.trunc(pendingScroll)));
    if (!lines || state.pane == null) return;
    pendingScroll -= lines;
    input({ pane: state.pane, kind: "scroll", lines });
  }, 50);
}

function setupProgramScroll() {
  const wrap = $("term-wrap");
  let lastY = null;
  wrap.addEventListener("touchstart", (e) => {
    lastY = e.touches.length === 1 ? e.touches[0].clientY : null;
    pendingScroll = 0;
  }, { passive: true });
  wrap.addEventListener("touchmove", (e) => {
    if (lastY == null || !programScrolls() || e.touches.length !== 1) return;
    const y = e.touches[0].clientY;
    // Finger down: back up through what came before, as on any page.
    scrollProgram(y - lastY);
    lastY = y;
    e.preventDefault();
  }, { passive: false });
  wrap.addEventListener("touchend", () => (lastY = null), { passive: true });
  wrap.addEventListener("wheel", (e) => {
    if (!programScrolls() || e.ctrlKey) return;
    const pixels = e.deltaMode === 1 ? e.deltaY * 1.25 * state.size : e.deltaY;
    scrollProgram(-pixels);
    e.preventDefault();
  }, { passive: false });
}

function toLatest() {
  const wrap = $("term-wrap");
  state.follow = true;
  wrap.scrollTop = wrap.scrollHeight;
  showToLatest();
}

// A row that is only a horizontal rule (Claude Code's dividers): kept on one line and cut at the
// screen's edge, instead of wrapping into several lines of dashes.
const RULE = /^[\s─━═╌╍┄┅┈┉\-_]+$/;

function renderRow(index) {
  const row = state.rows[index];
  if (!row) return;
  const runs = state.lines[index] || [];
  const cursor = cursorLine(state.screen);
  const cursorCol = cursor && cursor[1] === index ? cursor[0] : -1;
  const nodes = [];
  let col = 0;
  for (const run of runs) {
    const isWide = (run.s || 0) & 32;
    const chars = Array.from(run.t);
    const width = isWide ? 2 : chars.length;
    if (cursorCol >= col && cursorCol < col + width) {
      // Split the run around the cursor's character so the cursor sits on it whatever the wrapping.
      const at = isWide ? 0 : cursorCol - col;
      if (at > 0) nodes.push(runNode({ ...run, t: chars.slice(0, at).join("") }));
      const cell = runNode({ ...run, t: chars[at] || " " });
      cell.classList.add("cursor");
      nodes.push(cell);
      if (at + 1 < chars.length) nodes.push(runNode({ ...run, t: chars.slice(at + 1).join("") }));
    } else {
      nodes.push(runNode(run));
    }
    col += width;
  }
  if (cursorCol >= col) {
    // Past the end of the text: pad with spaces to the cursor.
    if (cursorCol > col) nodes.push(el("span", null, " ".repeat(cursorCol - col)));
    nodes.push(el("span", "cursor", " "));
  }
  row.replaceChildren(...nodes);
  row.classList.toggle("rule", runs.length > 0 && RULE.test(runs.map((r) => r.t).join("")));
}

function runNode(run) {
  const s = run.s || 0;
  const classes = [];
  if (s & 1) classes.push("b");
  if (s & 2) classes.push("i");
  if (s & 4) classes.push("u");
  if (s & 8) classes.push("s");
  if (s & 16) classes.push("d");
  if (s & 32) classes.push("w");
  const span = el("span", classes.join(" "), run.t);
  if (run.f != null) span.style.color = hex(run.f);
  if (run.b != null) span.style.background = hex(run.b);
  return span;
}

// The text stays at the chosen size; never sideways scrolling. The terminal is asked for the
// columns and lines that fill the page (and goes back to its pane's size in the app once no page
// shows it). Until it has them, or while another page asks for a different size, lines wider
// than the page wrap at its edge.
function layoutTerm() {
  const term = $("term");
  const width = $("term-wrap").clientWidth - 12;
  const cols = state.screen ? state.screen.cols : 80;
  const wrap = cols * cellRatio() * state.size > width + 0.5;
  // Only what changed is set: writing the same values on every screen update re-laid the page.
  const px = state.size + "px";
  if (term.style.fontSize !== px) term.style.fontSize = px;
  if (term.classList.contains("wrap") !== wrap) term.classList.toggle("wrap", wrap);
}

let sizeTimer = null;
/// Asks for the grid that fills the terminal's area, once it has settled (dragging a window
/// edge or a phone's keyboard sliding in sends a burst of resizes). Only the area decides, never
/// what arrives: sizing from the screen let a scrollbar coming and going (or a second page at
/// another size) bounce the terminal between two sizes, and an agent redraws on every change.
function fitTerm() {
  const wrapper = $("term-wrap");
  // The scrollbar's room is kept whether it shows or not (`scrollbar-gutter`), so this holds.
  const width = wrapper.clientWidth - 12;
  const height = wrapper.getBoundingClientRect().height - 12;
  if (state.pane == null || width <= 0 || height <= 0) return;
  const cols = Math.max(20, Math.min(400, Math.floor(width / (cellRatio() * state.size))));
  const rows = Math.max(5, Math.min(200, Math.floor(height / (1.25 * state.size))));
  const key = state.pane + ":" + cols + ":" + rows;
  if (key === state.sized) return;
  clearTimeout(sizeTimer);
  sizeTimer = setTimeout(() => {
    if (state.pane == null || !key.startsWith(state.pane + ":")) return;
    state.sized = key;
    input({ pane: state.pane, kind: "resize", cols, rows });
  }, 150);
}

let measuredRatio = null;
/// A terminal cell's width over the font size, measured once the terminal font is in (until then
/// a close guess).
function cellRatio() {
  return measuredRatio || 0.6;
}

function measureCell() {
  const probe = el("span", null, "0".repeat(100));
  probe.style.fontFamily = getComputedStyle($("term")).fontFamily;
  probe.style.fontSize = "100px";
  probe.style.position = "absolute";
  probe.style.visibility = "hidden";
  probe.style.whiteSpace = "pre";
  document.body.append(probe);
  const ratio = probe.getBoundingClientRect().width / 100 / 100;
  probe.remove();
  if (ratio > 0.3 && ratio < 1) measuredRatio = ratio;
  layoutTerm();
  fitTerm();
}

// Typing straight into the terminal: a hidden field takes the keyboard (phones need one), keys
// with a meaning of their own go as keys, text as text.
const KEY_NAMES = {
  Enter: "enter", Tab: "tab", Escape: "escape", Backspace: "backspace", Delete: "delete",
  ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right",
  Home: "home", End: "end", PageUp: "pageup", PageDown: "pagedown",
};

function setupKeyboard() {
  const keyboard = $("keyboard");
  // On a touch screen a tap on the terminal goes to the prompt box below it: the phone's keyboard
  // is for writing a prompt there, not for typing into the terminal key by key. With a mouse and
  // a keyboard, typing goes straight to the terminal.
  $("term").addEventListener("click", () => {
    if (matchMedia("(pointer: coarse)").matches) $("prompt").focus({ preventScroll: true });
    else keyboard.focus({ preventScroll: true });
  });
  // Korean, Japanese and Chinese are typed through the input method. Its events are not to be
  // trusted for the text (Safari's compositionend can carry a half-built syllable, and the next
  // composition may start before the last one settles), so the field itself is the record:
  // `sent` is how much of its value has gone to the terminal, and `start` where the composition
  // now under way begins. Whatever lies between is committed and goes out; the field is emptied
  // only when nothing is being composed, so the input method is never cut off mid-syllable.
  let composing = false;
  let start = 0;
  let sent = 0;
  const flush = (clear = true) => {
    const value = keyboard.value;
    const end = composing ? Math.min(start, value.length) : value.length;
    if (end > sent) sendText(value.slice(sent, end));
    sent = Math.max(sent, end);
    if (clear && !composing && sent >= value.length) {
      keyboard.value = "";
      sent = 0;
    }
  };
  keyboard.addEventListener("compositionstart", () => {
    // Send what came before, but leave the field as it is: the input method is about to write
    // into it.
    flush(false);
    composing = true;
    start = keyboard.value.length;
  });
  keyboard.addEventListener("compositionend", () => {
    composing = false;
    // The field holds the final text only after this event has run its course.
    setTimeout(flush, 0);
  });
  keyboard.addEventListener("keydown", (e) => {
    if (composing || e.isComposing || e.keyCode === 229) return;
    const named = KEY_NAMES[e.key] || (/^F([1-9]|1[0-2])$/.test(e.key) ? e.key.toLowerCase() : null);
    if (named) {
      e.preventDefault();
      flush();
      sendKey(named, { ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey });
      return;
    }
    if ((e.ctrlKey || e.altKey) && e.key.length === 1) {
      e.preventDefault();
      flush();
      sendKey(e.key.toLowerCase(), { ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey });
    }
  });
  keyboard.addEventListener("beforeinput", (e) => {
    if (composing || e.isComposing) return;
    // Phones send these instead of key events.
    if (e.inputType === "deleteContentBackward") {
      e.preventDefault();
      flush();
      sendKey("backspace");
    } else if (e.inputType === "insertLineBreak" || e.inputType === "insertParagraph") {
      e.preventDefault();
      flush();
      sendKey("enter");
    }
  });
  // Plain typing (no composition) goes as it comes; composed text waits for its compositionend.
  keyboard.addEventListener("input", (e) => {
    if (composing || e.isComposing) return;
    flush();
  });
}

function setupChat() {
  $("chat-toggle").addEventListener("click", () => {
    state.chatView = !state.chatView;
    render();
  });
}

function setupPrompt() {
  const prompt = $("prompt");
  prompt.placeholder = T.promptHint;
  $("send-button").textContent = T.send;
  const grow = () => {
    prompt.style.height = "auto";
    prompt.style.height = Math.min(prompt.scrollHeight, window.innerHeight * 0.3) + "px";
  };
  prompt.addEventListener("input", grow);
  prompt.addEventListener("keydown", (e) => {
    // Enter sends on a computer; Shift+Enter (or a phone's return key) adds a line.
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing && matchMedia("(pointer: fine)").matches) {
      e.preventDefault();
      $("prompt-form").requestSubmit();
    }
  });
  $("prompt-form").addEventListener("submit", (e) => {
    e.preventDefault();
    const text = prompt.value;
    if (state.pane == null) return;
    if (attachments.some((a) => !a.path && !a.failed)) return alert(T.attachWait);
    const images = attachments.filter((a) => a.path).map((a) => a.path);
    if (!text.trim() && !images.length) return;
    // To a chat's lead, as the chat sends it (its instructions go in front the first time).
    input({ pane: state.pane, kind: inChat() ? "chat" : "prompt", text, images });
    prompt.value = "";
    clearAttachments();
    grow();
  });
}

// ---- Images ----
//
// Attached from the button, pasted into the prompt box or dropped on it. Each is uploaded to the
// Mac at once (made smaller first when it is a large photo) and sent with the next prompt, which
// the Mac pastes in front of it as a drop on the terminal would.

const ATTACH_MAX_SIDE = 2000;
const ATTACH_MAX_BYTES = 4 * 1024 * 1024;
const ATTACH_MAX = 10;
const attachments = []; // { url, path, failed }

function setupAttachments() {
  const btn = $("attach-button");
  const picker = $("attach-input");
  btn.setAttribute("aria-label", T.attach);
  btn.title = T.attach;
  btn.addEventListener("click", () => picker.click());
  picker.addEventListener("change", () => {
    [...picker.files].forEach(addAttachment);
    picker.value = "";
  });
  $("prompt").addEventListener("paste", (e) => {
    const files = [...(e.clipboardData?.files || [])].filter((f) => f.type.startsWith("image/"));
    if (!files.length) return;
    e.preventDefault();
    files.forEach(addAttachment);
  });
  const form = $("prompt-form");
  form.addEventListener("dragover", (e) => {
    if (![...(e.dataTransfer?.types || [])].includes("Files")) return;
    e.preventDefault();
    form.classList.add("dragging");
  });
  form.addEventListener("dragleave", () => form.classList.remove("dragging"));
  form.addEventListener("drop", (e) => {
    form.classList.remove("dragging");
    const files = [...(e.dataTransfer?.files || [])].filter((f) => f.type.startsWith("image/"));
    if (!files.length) return;
    e.preventDefault();
    files.forEach(addAttachment);
  });
}

// A photo as the agent needs it: at most ATTACH_MAX_SIDE pixels a side and a few MB, in a format
// it reads (PNG, JPEG, GIF, WebP; anything else, such as HEIC, becomes JPEG). Null if the browser
// can't read it as an image.
async function shrinkImage(file) {
  const readable = ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(file.type);
  let bitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch (_) {
    return readable && file.size <= ATTACH_MAX_BYTES ? file : null;
  }
  const scale = Math.min(1, ATTACH_MAX_SIDE / Math.max(bitmap.width, bitmap.height));
  if (readable && scale === 1 && file.size <= ATTACH_MAX_BYTES) {
    bitmap.close();
    return file;
  }
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(bitmap.width * scale));
  canvas.height = Math.max(1, Math.round(bitmap.height * scale));
  const ctx = canvas.getContext("2d");
  ctx.fillStyle = "#fff"; // JPEG has no transparency
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  bitmap.close();
  return new Promise((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.85));
}

function addAttachment(file) {
  if (state.pane == null || attachments.length >= ATTACH_MAX || !file.type.startsWith("image/")) return;
  const item = { url: URL.createObjectURL(file), path: null, failed: false };
  attachments.push(item);
  renderAttachments();
  const pane = state.pane;
  (async () => {
    const blob = await shrinkImage(file);
    if (!blob) throw new Error("not an image");
    // Raw bytes; the JSON content type only satisfies the same-origin (CSRF) check, as for voice.
    const res = await fetch(`/api/attach?pane=${pane}`, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", "X-Agentty": "1" },
      body: blob,
    });
    if (res.status === 401) showLogin();
    if (!res.ok) throw new Error(String(res.status));
    const data = await res.json();
    if (!data.path) throw new Error("no path");
    item.path = data.path;
  })()
    .catch(() => { item.failed = true; })
    .finally(renderAttachments);
}

function removeAttachment(item) {
  const at = attachments.indexOf(item);
  if (at < 0) return;
  attachments.splice(at, 1);
  URL.revokeObjectURL(item.url);
  renderAttachments();
}

function clearAttachments() {
  attachments.splice(0).forEach((a) => URL.revokeObjectURL(a.url));
  renderAttachments();
}

function renderAttachments() {
  const box = $("attachments");
  box.replaceChildren(...attachments.map((item) => {
    const chip = document.createElement("div");
    chip.className = "attachment" + (item.failed ? " failed" : item.path ? "" : " pending");
    if (item.failed) chip.title = T.attachFailed;
    const img = document.createElement("img");
    img.src = item.url;
    img.alt = "";
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "remove";
    remove.textContent = "×";
    remove.setAttribute("aria-label", T.attachRemove);
    remove.addEventListener("click", () => removeAttachment(item));
    chip.append(img, remove);
    return chip;
  }));
  box.hidden = !attachments.length;
}

// ---- Voice ----
//
// Record a short clip in the browser, upload it to the Mac, and drop the transcript into the
// prompt box for the person to check and send. The audio is 16-bit PCM WAV captured through
// WebAudio (not MediaRecorder's Opus), so the Mac needs no audio decoder. Nothing is uploaded until
// the person stops recording, and the transcript only fills the box — it is never auto-sent.
//
// The clip is sent at 16 kHz, what whisper uses, not the browser's own rate (often 48 kHz): a
// third of the bytes to push over a phone's uplink. Recording stops by itself just short of the
// Mac's two-minute cap, so nothing the person said is cut off unseen.

const VOICE_RATE = 16000;
const MAX_RECORD_SECONDS = 110;

let recorder = null; // { stream, ctx, node, source, chunks, rate, samples } while recording
let voiceStarting = false; // guards the getUserMedia await against a second tap

function setupVoice() {
  const btn = $("mic-button");
  btn.setAttribute("aria-label", T.micStart);
  btn.title = T.micStart;
  btn.addEventListener("click", () => {
    if (btn.disabled || voiceStarting) return;
    if (recorder) finishVoice(btn);
    else beginVoice(btn);
  });
  // A recording must never outlive the moment: if the tab is hidden or closed, stop the mic so it
  // can't stay on with no button to stop it.
  window.addEventListener("pagehide", stopRecording);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) stopRecording();
  });
}

// Tear down an in-progress recording without transcribing — used when the pane goes away, the
// user navigates, or the tab is hidden. Safe to call when nothing is recording.
function stopRecording() {
  const rec = recorder;
  recorder = null;
  if (!rec) return;
  try {
    rec.node.onaudioprocess = null;
    rec.node.disconnect();
    rec.source.disconnect();
  } catch (_) {
    /* already gone */
  }
  rec.stream.getTracks().forEach((t) => t.stop());
  rec.ctx.close().catch(() => {});
  const btn = $("mic-button");
  if (btn) {
    btn.classList.remove("recording");
    btn.setAttribute("aria-pressed", "false");
    btn.setAttribute("aria-label", T.micStart);
    btn.title = T.micStart;
  }
}

async function beginVoice(btn) {
  if (state.pane == null) return;
  voiceStarting = true;
  try {
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
    });
    // The pane may have gone while the permission prompt was up.
    if (state.pane == null) {
      stream.getTracks().forEach((t) => t.stop());
      return;
    }
    const Ctx = window.AudioContext || window.webkitAudioContext;
    const ctx = new Ctx();
    // iOS Safari starts the context suspended; without this the capture is silent.
    if (ctx.state === "suspended") {
      try {
        await ctx.resume();
      } catch (_) {
        /* best effort */
      }
    }
    const source = ctx.createMediaStreamSource(stream);
    const node = ctx.createScriptProcessor(4096, 1, 1);
    const rec = { stream, ctx, node, source, chunks: [], rate: ctx.sampleRate, samples: 0 };
    node.onaudioprocess = (e) => {
      const chunk = new Float32Array(e.inputBuffer.getChannelData(0));
      rec.chunks.push(chunk);
      rec.samples += chunk.length;
      // Out of the event, so the node is not torn down from inside its own callback.
      if (rec.samples >= MAX_RECORD_SECONDS * rec.rate) setTimeout(() => recorder === rec && finishVoice(btn));
    };
    source.connect(node);
    node.connect(ctx.destination);
    recorder = rec;
    btn.classList.add("recording");
    btn.setAttribute("aria-pressed", "true");
    btn.setAttribute("aria-label", T.micRecording);
    btn.title = T.micRecording;
  } catch (_) {
    flashMic(btn, T.micDenied);
  } finally {
    voiceStarting = false;
  }
}

async function finishVoice(btn) {
  const rec = recorder;
  recorder = null;
  if (!rec) return;
  btn.classList.remove("recording");
  btn.setAttribute("aria-pressed", "false");
  rec.node.onaudioprocess = null;
  rec.node.disconnect();
  rec.source.disconnect();
  rec.stream.getTracks().forEach((t) => t.stop());
  const blob = pcmToWav(resample(flatten(rec.chunks), rec.rate, VOICE_RATE), VOICE_RATE);
  rec.ctx.close().catch(() => {});
  const pane = state.pane;
  if (pane == null) return;

  btn.disabled = true;
  btn.classList.add("busy");
  btn.setAttribute("aria-label", T.micBusy);
  btn.title = T.micBusy;
  const controller = new AbortController();
  // Room for a slow uplink (the Mac waits up to a minute for the clip) and the transcription.
  const timer = setTimeout(() => controller.abort(), 120000);
  try {
    // No language hint: the page's UI language says nothing about the language spoken, so the
    // Mac detects it.
    const q = `?pane=${pane}`;
    // The body is a WAV blob; the JSON content type is only here to satisfy the same-origin
    // (CSRF) check — the server reads /api/voice as raw bytes, not as JSON.
    const res = await fetch("/api/voice" + q, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", "X-Agentty": "1" },
      body: blob,
      signal: controller.signal,
    });
    if (res.status === 401) return showLogin(); // session expired: bounce to sign-in, like other calls
    if (res.status === 429) {
      // Another clip is being transcribed (this page's or another device's).
      flashMic(btn, T.micTooBusy);
      return;
    }
    if (!res.ok) throw new Error(String(res.status));
    const data = await res.json();
    const text = (data.text || "").trim();
    // The user may have switched terminals while transcribing; don't drop the text into the wrong
    // prompt box.
    if (text && state.pane === pane) {
      const prompt = $("prompt");
      prompt.value = prompt.value ? prompt.value.replace(/\s*$/, "") + " " + text : text;
      prompt.dispatchEvent(new Event("input"));
      prompt.focus();
    }
    btn.setAttribute("aria-label", T.micStart);
    btn.title = T.micStart;
  } catch (_) {
    flashMic(btn, T.micFailed);
  } finally {
    clearTimeout(timer);
    btn.disabled = false;
    btn.classList.remove("busy");
  }
}

function flatten(chunks) {
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Float32Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  return out;
}

// `samples` from `from` Hz to `to` Hz. Going down, each output sample is the average of the input
// samples it covers (a crude low-pass, enough for speech); going up, it is linearly interpolated.
function resample(samples, from, to) {
  if (!from || from === to) return samples;
  const ratio = from / to;
  const out = new Float32Array(Math.floor(samples.length / ratio));
  for (let i = 0; i < out.length; i++) {
    const start = i * ratio;
    if (ratio < 1) {
      const j = Math.floor(start);
      const a = samples[j];
      const b = j + 1 < samples.length ? samples[j + 1] : a;
      out[i] = a + (b - a) * (start - j);
    } else {
      const end = Math.min(Math.ceil(start + ratio), samples.length);
      let sum = 0;
      let n = 0;
      for (let j = Math.floor(start); j < end; j++, n++) sum += samples[j];
      out[i] = n ? sum / n : 0;
    }
  }
  return out;
}

// 16-bit PCM mono WAV at `rate` (the page sends 16 kHz; the Mac resamples anything else).
function pcmToWav(samples, rate) {
  const n = samples.length;
  const buf = new ArrayBuffer(44 + n * 2);
  const v = new DataView(buf);
  const tag = (o, s) => {
    for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i));
  };
  tag(0, "RIFF");
  v.setUint32(4, 36 + n * 2, true);
  tag(8, "WAVE");
  tag(12, "fmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  tag(36, "data");
  v.setUint32(40, n * 2, true);
  let o = 44;
  for (let i = 0; i < n; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    v.setInt16(o, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    o += 2;
  }
  return new Blob([buf], { type: "audio/wav" });
}

function flashMic(btn, message) {
  btn.classList.remove("recording", "busy");
  btn.setAttribute("aria-pressed", "false");
  btn.title = message;
  btn.setAttribute("aria-label", message);
  setTimeout(() => {
    btn.title = T.micStart;
    btn.setAttribute("aria-label", T.micStart);
  }, 2500);
}

// ---- Notifications ----

function notifyChanges(sessions) {
  const before = state.previous;
  state.previous = new Map(sessions.map((s) => [s.pane, s]));
  if (!before.size || !("Notification" in window) || Notification.permission !== "granted") return;
  if (!document.hidden) return;
  for (const s of sessions) {
    const old = before.get(s.pane);
    if (!old) continue;
    let what = null;
    if (s.needs_user && !old.needs_user) what = T.asks;
    else if (old.working && !s.working && !s.needs_user) what = T.finished;
    if (!what) continue;
    // The session's name only, never what the agent asks: a notification can show on a locked
    // phone's screen, and that text may hold anything the agent read.
    const n = new Notification(`${what} · ${s.workspace}`, { body: s.title || "", tag: "agentty-" + s.pane });
    n.onclick = () => {
      window.focus();
      selectPane(s.pane);
      n.close();
    };
  }
}

function renderNotifyButton() {
  const button = $("notify-button");
  if (!("Notification" in window)) {
    button.hidden = true;
    return;
  }
  const granted = Notification.permission === "granted";
  button.textContent = granted ? T.notifyOn : T.notify;
  button.disabled = granted || Notification.permission === "denied";
}

// ---- Start ----

async function start() {
  let me;
  try {
    me = (await api("/api/me")).data;
  } catch (_) {
    showLogin();
    $("login-error").textContent = T.failed;
    return;
  }
  if (!me.signedIn) return showLogin(me.lockedFor, me);
  $("login").hidden = true;
  $("app").hidden = false;
  renderNotifyButton();
  $("logout-button").textContent = T.signOut;
  // The overview comes first on the stream; the terminal follows once a pane is chosen.
  listen();
}

window.addEventListener("DOMContentLoaded", () => {
  $("login-form").addEventListener("submit", signIn);
  setupPasswordBox();
  // A notification's link opened in a page already showing: only the address's `#pane` changed.
  window.addEventListener("hashchange", () => {
    if (state.started && Number((location.hash || "").slice(1)) !== state.pane) openAskedPane();
  });
  $("logout-button").addEventListener("click", signOut);
  $("menu-button").addEventListener("click", () => openDrawer("sidebar"));
  $("panes-button").addEventListener("click", () => openDrawer("panes"));
  $("scrim").addEventListener("click", () => {
    closeDrawers();
    closeSheet();
    closeFolderSheet();
  });
  $("home-label").textContent = T.home;
  $("home-link").addEventListener("click", () => {
    closeDrawers();
    goHome();
  });
  $("new-cancel").addEventListener("click", closeSheet);
  $("folder-cancel").addEventListener("click", closeFolderSheet);
  $("folder-create").addEventListener("click", createFolder);
  $("folder-new-name").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      createFolder();
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && state.sheetFor != null) closeSheet();
    if (e.key === "Escape" && state.folderSheet) closeFolderSheet();
  });
  $("notify-button").addEventListener("click", async () => {
    await Notification.requestPermission();
    renderNotifyButton();
  });
  document.fonts.load('13px "Agentty Mono"').then(measureCell, measureCell);
  window.addEventListener("resize", () => {
    if (wide()) closeDrawers();
    layoutTerm();
  });
  // The area changes with the window, the keyboard on a phone, and the bars around it.
  new ResizeObserver(() => fitTerm()).observe($("term-wrap"), { box: "border-box" });
  $("term-wrap").addEventListener("scroll", onTermScroll, { passive: true });
  $("to-latest").addEventListener("click", toLatest);
  $("to-latest").textContent = T.toLatest;
  setupProgramScroll();
  setupKeyboard();
  setupPrompt();
  setupAttachments();
  setupChat();
  setupVoice();
  start();
});
