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
    signIn: "Sign in", password: "Password",
    wrong: "Wrong password.", locked: "Too many wrong passwords. Try again in {s} s.", noPassword: "No password is set in Agentty yet.",
    failed: "Couldn't reach Agentty.", signOut: "Sign out", notify: "Notify me", notifyOn: "Notifying",
    noWorkspaces: "No workspaces are open in Agentty.", sleeping: "This workspace is asleep. Wake it up to start its terminals again.",
    send: "Send", promptHint: "Prompt for this terminal…", live: "Connected", offline: "Reconnecting…",
    privacy: "Privacy Policy", terms: "Terms of Service", eula: "License Agreement", device: "Device", ip: "Tailnet IP", account: "Account", target: "Connecting to", secure: "Security", secureValue: "Tailscale (WireGuard) · HTTPS", browser: "Browser", shownOnly: "Shown here only; nothing is stored.",
    tooLong: "Too long.", busy: "Agentty is busy; try again.", finished: "Finished", asks: "Asks you", panes: "Split panes",
    home: "Home", needsYou: "Needs you", workingNow: "Working", workspacesTitle: "Workspaces", plugins: "Plugins",
    nothingWaiting: "Nothing is waiting for you.", nothingWorking: "No agent is working right now.", noPlugins: "No plugins installed.",
    wake: "Wake up", waking: "Waking…", newTab: "New tab", newIn: "New tab in {ws}", cancel: "Cancel",
    terminal: "Terminal", claude: "Claude Code", codex: "Codex", tooManyTabs: "Too many new tabs at once. Wait a minute.",
    open: "{n} open", asleep: "Asleep", running: "{n} running",
    pstate: { running: "Running", starting: "Starting", stopped: "Stopped", failed: "Stopped with an error", consent: "Waiting for your OK", off: "Off" },
    astate: { working: "Working", idle: "Idle", error: "Error" },
  },
  ko: {
    signIn: "로그인", password: "비밀번호",
    wrong: "비밀번호가 틀렸습니다.", locked: "비밀번호가 여러 번 틀렸습니다. {s}초 뒤에 다시 시도하세요.", noPassword: "Agentty에 아직 비밀번호가 설정되지 않았습니다.",
    failed: "Agentty에 연결하지 못했습니다.", signOut: "로그아웃", notify: "알림 받기", notifyOn: "알림 켜짐",
    noWorkspaces: "Agentty에 열린 작업공간이 없습니다.", sleeping: "쉬고 있는 작업공간입니다. 깨우면 터미널이 다시 시작됩니다.",
    send: "보내기", promptHint: "이 터미널에 보낼 프롬프트…", live: "연결됨", offline: "다시 연결하는 중…",
    privacy: "개인정보처리방침", terms: "이용약관", eula: "라이선스 계약", device: "접속 기기", ip: "Tailnet IP", account: "계정", target: "접속 대상", secure: "보안", secureValue: "Tailscale(WireGuard) 암호화 · HTTPS", browser: "브라우저", shownOnly: "이 화면에 보여 주기만 하며 저장하지 않습니다.",
    tooLong: "너무 깁니다.", busy: "Agentty가 바쁩니다. 다시 시도하세요.", finished: "작업 완료", asks: "요청", panes: "분할창",
    home: "홈", needsYou: "응답 대기", workingNow: "작업 중", workspacesTitle: "작업공간", plugins: "플러그인",
    nothingWaiting: "기다리는 요청이 없습니다.", nothingWorking: "지금 작업 중인 에이전트가 없습니다.", noPlugins: "설치된 플러그인이 없습니다.",
    wake: "깨우기", waking: "깨우는 중…", newTab: "새 탭", newIn: "{ws}에 새 탭", cancel: "취소",
    terminal: "터미널", claude: "Claude Code", codex: "Codex", tooManyTabs: "새 탭을 너무 많이 열었습니다. 1분 뒤에 다시 시도하세요.",
    open: "{n}개 열림", asleep: "쉬는 중", running: "{n}개 실행 중",
    pstate: { running: "실행 중", starting: "시작하는 중", stopped: "멈춤", failed: "오류로 멈춤", consent: "허용 대기", off: "꺼짐" },
    astate: { working: "작업 중", idle: "대기", error: "오류" },
  },
  ja: {
    signIn: "サインイン", password: "パスワード",
    wrong: "パスワードが違います。", locked: "パスワードの誤りが多すぎます。{s} 秒後に再試行してください。", noPassword: "Agentty にまだパスワードが設定されていません。",
    failed: "Agentty に接続できませんでした。", signOut: "サインアウト", notify: "通知を受け取る", notifyOn: "通知オン",
    noWorkspaces: "Agentty で開いているワークスペースはありません。", sleeping: "休止中のワークスペースです。起こすとターミナルがまた始まります。",
    send: "送信", promptHint: "このターミナルへのプロンプト…", live: "接続中", offline: "再接続しています…",
    privacy: "プライバシーポリシー", terms: "利用規約", eula: "ライセンス契約", device: "接続端末", ip: "Tailnet IP", account: "アカウント", target: "接続先", secure: "セキュリティ", secureValue: "Tailscale（WireGuard）暗号化 · HTTPS", browser: "ブラウザ", shownOnly: "表示するだけで、保存はしません。",
    tooLong: "長すぎます。", busy: "Agentty が混み合っています。もう一度お試しください。", finished: "完了", asks: "質問", panes: "分割ペイン",
    home: "ホーム", needsYou: "応答待ち", workingNow: "作業中", workspacesTitle: "ワークスペース", plugins: "プラグイン",
    nothingWaiting: "待っている質問はありません。", nothingWorking: "いま作業中のエージェントはありません。", noPlugins: "インストール済みのプラグインはありません。",
    wake: "起こす", waking: "起こしています…", newTab: "新しいタブ", newIn: "{ws} に新しいタブ", cancel: "キャンセル",
    terminal: "ターミナル", claude: "Claude Code", codex: "Codex", tooManyTabs: "新しいタブを開きすぎました。1 分後にもう一度お試しください。",
    open: "{n} 件 開いています", asleep: "休止中", running: "{n} 件 実行中",
    pstate: { running: "実行中", starting: "起動中", stopped: "停止", failed: "エラーで停止", consent: "許可待ち", off: "オフ" },
    astate: { working: "作業中", idle: "待機", error: "エラー" },
  },
  zh: {
    signIn: "登录", password: "密码",
    wrong: "密码错误。", locked: "密码错误次数过多，请在 {s} 秒后重试。", noPassword: "Agentty 尚未设置密码。",
    failed: "无法连接 Agentty。", signOut: "退出登录", notify: "接收通知", notifyOn: "通知已开启",
    noWorkspaces: "Agentty 中没有打开的工作区。", sleeping: "这个工作区处于休眠状态。唤醒后终端会重新启动。",
    send: "发送", promptHint: "发送给此终端的提示…", live: "已连接", offline: "正在重新连接…",
    privacy: "隐私政策", terms: "服务条款", eula: "许可协议", device: "接入设备", ip: "Tailnet IP", account: "账号", target: "连接目标", secure: "安全", secureValue: "Tailscale（WireGuard）加密 · HTTPS", browser: "浏览器", shownOnly: "仅在此显示，不会保存。",
    tooLong: "太长了。", busy: "Agentty 正忙，请重试。", finished: "已完成", asks: "询问", panes: "分屏",
    home: "主页", needsYou: "等待回应", workingNow: "工作中", workspacesTitle: "工作区", plugins: "插件",
    nothingWaiting: "没有等待你的请求。", nothingWorking: "现在没有正在工作的代理。", noPlugins: "没有安装插件。",
    wake: "唤醒", waking: "正在唤醒…", newTab: "新标签页", newIn: "在 {ws} 中新建标签页", cancel: "取消",
    terminal: "终端", claude: "Claude Code", codex: "Codex", tooManyTabs: "新建的标签页太多了，请一分钟后再试。",
    open: "{n} 个已打开", asleep: "休眠中", running: "{n} 个运行中",
    pstate: { running: "运行中", starting: "正在启动", stopped: "已停止", failed: "因错误停止", consent: "等待你的允许", off: "已关闭" },
    astate: { working: "工作中", idle: "空闲", error: "错误" },
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
  tab: 0,
  pane: null,
  events: null,
  rows: [],
  lines: [],
  screen: null,
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
  // The size last asked of the terminal being looked at ("pane:cols:rows").
  sized: null,
};

const session = (pane) => state.sessions.find((s) => s.pane === pane);
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
    const { status, data } = await api("/api/login", { password: input.value });
    input.value = "";
    input.blur();
    if (status === 200) return start();
    if (status === 429) return lockedFor(data.lockedFor || 60);
    $("login-error").textContent = status === 401 ? T.wrong : status === 409 ? T.noPassword : T.failed;
  } catch (_) {
    $("login-error").textContent = T.failed;
  }
  button.disabled = false;
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
  const asked = Number((location.hash || "").slice(1));
  if (asked && Number.isInteger(asked) && session(asked)) return selectPane(asked);
  goHome();
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
  state.pane = pane;
  if (changed) {
    state.rows = [];
    state.lines = [];
    state.screen = null;
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
  redraw("body", [state.workspace, state.workspaces.length, ws && ws.sleeping, state.waking, shown(session(state.pane)), state.ctrl, state.size], renderBody);
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

function renderWorkspaces() {
  const root = $("workspaces");
  root.replaceChildren();
  if (!state.workspaces.length) {
    root.append(el("div", "empty-note", T.noWorkspaces));
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
  for (const id of ["term-wrap", "keys", "prompt-form"]) $(id).hidden = home || !!note || !s;
  $("asks").hidden = !(s && s.asks);
  $("asks").textContent = (s && s.asks) || "";
  renderKeys(s && s.needs_user);
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
  add("⏎", "enter");
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

function onScreen(update) {
  if (update.pane !== state.pane) return;
  const term = $("term");
  if (update.full || !state.screen || state.screen.cols !== update.cols || state.screen.rows !== update.rows) {
    term.replaceChildren();
    state.rows = [];
    state.lines = [];
    for (let i = 0; i < update.rows; i++) {
      const row = el("div", "row");
      term.append(row);
      state.rows.push(row);
      state.lines.push([]);
    }
  }
  const before = state.screen && state.screen.cursor;
  state.screen = update;
  term.style.background = hex(update.bg);
  term.style.color = hex(update.fg);
  // The frame around it too, so a scrollbar's gutter shows no stripe of another color.
  $("term-wrap").style.background = hex(update.bg);
  const dirty = new Set();
  for (const [index, runs] of update.lines) {
    if (index >= state.rows.length) continue;
    state.lines[index] = runs;
    dirty.add(index);
  }
  // The cursor is drawn inside its row's text, so a row it left or entered is drawn again too.
  if (before) dirty.add(before[1]);
  if (update.cursor) dirty.add(update.cursor[1]);
  for (const index of dirty) renderRow(index);
  // Empty rows under the last text (or the cursor) are left out: a shell's prompt sits at the
  // top of a tall screen, and scrolling to the bottom would show nothing but blank rows.
  let last = update.cursor ? update.cursor[1] : 0;
  state.lines.forEach((runs, index) => {
    if (runs.some((r) => r.t.trim() || r.b != null)) last = Math.max(last, index);
  });
  state.rows.forEach((row, index) => (row.hidden = index > last));
  layoutTerm();
  const wrap = $("term-wrap");
  wrap.scrollTop = wrap.scrollHeight;
}

// A row that is only a horizontal rule (Claude Code's dividers): kept on one line and cut at the
// screen's edge, instead of wrapping into several lines of dashes.
const RULE = /^[\s─━═╌╍┄┅┈┉\-_]+$/;

function renderRow(index) {
  const row = state.rows[index];
  if (!row) return;
  const runs = state.lines[index] || [];
  const cursor = state.screen && state.screen.cursor;
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
  $("term").addEventListener("click", () => keyboard.focus({ preventScroll: true }));
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
    if (!text.trim() || state.pane == null) return;
    input({ pane: state.pane, kind: "prompt", text });
    prompt.value = "";
    grow();
  });
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
  $("logout-button").addEventListener("click", signOut);
  $("menu-button").addEventListener("click", () => openDrawer("sidebar"));
  $("panes-button").addEventListener("click", () => openDrawer("panes"));
  $("scrim").addEventListener("click", () => {
    closeDrawers();
    closeSheet();
  });
  $("home-label").textContent = T.home;
  $("home-link").addEventListener("click", () => {
    closeDrawers();
    goHome();
  });
  $("new-cancel").addEventListener("click", closeSheet);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && state.sheetFor != null) closeSheet();
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
  setupKeyboard();
  setupPrompt();
  start();
});
