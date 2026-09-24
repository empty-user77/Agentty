// Types for agentty-plugin.mjs (Agentty plugin SDK for Node.js).
// SPDX-License-Identifier: MIT

export const SDK_VERSION: string;
export const API_VERSION: number;

export type Gap = 'none' | 'small' | 'medium' | 'large';
export type TextStyle = 'body' | 'title' | 'muted' | 'small' | 'code' | 'error' | 'success';
export type Variant = 'primary' | 'secondary' | 'ghost' | 'danger';
export type Tone = 'neutral' | 'info' | 'success' | 'warning' | 'error';

export interface ItemAction { id: string; label?: string; icon?: string; tooltip?: string }
export interface ListItem { id: string; title: string; subtitle?: string; detail?: string; icon?: string; actions?: ItemAction[] }
export interface ChoiceOption { value: string; label: string }

export type UiNode =
  | { type: 'column'; children: UiNode[]; gap?: Gap }
  | { type: 'row'; children: UiNode[]; gap?: Gap; wrap?: boolean }
  | { type: 'section'; title: string; children: UiNode[] }
  | { type: 'text'; text: string; style?: TextStyle }
  | { type: 'button'; id: string; label: string; icon?: string; variant?: Variant; disabled?: boolean }
  | { type: 'input'; id: string; placeholder?: string; value?: string }
  | { type: 'list'; id: string; items: ListItem[]; empty?: string }
  | { type: 'choice'; id: string; options: ChoiceOption[]; value?: string }
  | { type: 'toggle'; id: string; label: string; value?: boolean }
  | { type: 'badge'; text: string; tone?: Tone }
  | { type: 'spinner'; text?: string }
  | { type: 'divider' };

type Child = UiNode | null | undefined | false | Child[];

export const ui: {
  column(children: Child[], options?: { gap?: Gap }): UiNode;
  row(children: Child[], options?: { gap?: Gap; wrap?: boolean }): UiNode;
  section(title: string, children: Child[]): UiNode;
  text(text: string, style?: TextStyle): UiNode;
  button(id: string, label: string, options?: { icon?: string; variant?: Variant; disabled?: boolean }): UiNode;
  input(id: string, options?: { placeholder?: string; value?: string; rows?: number }): UiNode;
  list(id: string, items: ListItem[], options?: { empty?: string }): UiNode;
  choice(id: string, options: ChoiceOption[], value?: string): UiNode;
  toggle(id: string, label: string, value?: boolean): UiNode;
  badge(text: string, tone?: Tone): UiNode;
  spinner(text?: string): UiNode;
  divider(): UiNode;
};

export type AgentStatus = 'shell' | 'idle' | 'working' | 'thinking' | 'finished' | 'permission' | 'question' | 'interrupted' | 'exited';

export interface PaneInfo {
  id: number;
  /** claude | codex | shell */
  kind: 'claude' | 'codex' | 'shell';
  /** CLI running in the pane (claude, codex, gemini, …, or shell). */
  tool: string;
  title: string;
  cwd: string;
  sessionId?: string | null;
  status: AgentStatus;
  running: boolean;
}

export interface WorkspaceInfo { id: number; name: string; cwd: string; active: boolean; panes?: PaneInfo[] }

export interface Context {
  workspace: WorkspaceInfo | null;
  pane: PaneInfo | null;
  /** en | ko | ja | zh */
  language: string;
}

export interface InitializeParams {
  apiVersion: number;
  agentty: { version: string };
  plugin: { id: string; name: string; version: string; dir: string; dataDir: string };
  language: string;
  context: Context;
}

export interface UiEvent {
  element: string;
  /** click | change | submit | select | action */
  event: string;
  value?: unknown;
  item?: string;
  action?: string;
  context: Context;
}

export interface UrlOpen { path: string; query: Record<string, string>; url: string }

export type PromptTarget = 'ask' | 'active' | 'newWorkspace' | 'newTab' | 'split' | 'pane' | 'workspace' | 'own';

export interface PromptRequest {
  text: string;
  title?: string;
  target?: PromptTarget;
  paneId?: number;
  workspaceId?: number;
  /** With target 'own': the automation (tab) the job opens beside. */
  instance?: string;
  agent?: 'claude' | 'codex' | 'shell';
  cwd?: string;
  submit?: boolean;
}

export interface Session {
  paneId: number;
  agent: string;
  sessionId: string;
  title: string;
  cwd: string | null;
  status: AgentStatus;
  turnCount: number;
  turns: { role: 'user' | 'assistant'; text: string }[];
}

export type BrowserMode = 'auto' | 'background' | 'visible';

export interface SiteStatus {
  host: string;
  /** null when the site's manifest entry names no `signedInCookie`. */
  signedIn: boolean | null;
  /** When the sign-in ends (ms since the epoch); null for a session that ends with the browser. */
  expiresAt: number | null;
}

export interface BrowserTabInfo {
  tabId: number;
  url: string | null;
  title: string | null;
  loading: boolean;
  visible: boolean;
  /** The plugin's site the page is on, or null when it is elsewhere (scripts are refused there). */
  site: string | null;
}

export interface Browser {
  sites(options?: { profile?: string }): Promise<SiteStatus[]>;
  open(url: string, options?: { mode?: BrowserMode; profile?: string; instance?: string }): Promise<{ tabId: number }>;
  navigate(tabId: number, url: string): Promise<void>;
  eval<T = unknown, A = unknown>(tabId: number, script: string | ((args: A) => T | Promise<T>), args?: A, options?: { timeoutMs?: number }): Promise<T>;
  wait(tabId: number, options?: { timeoutMs?: number }): Promise<{ url: string; title: string | null }>;
  info(tabId: number): Promise<BrowserTabInfo>;
  show(tabId: number, message?: string): Promise<void>;
  hide(tabId: number): Promise<void>;
  close(tabId: number): Promise<void>;
  signIn(host: string, options?: { message?: string; profile?: string; instance?: string }): Promise<{ tabId: number; signedIn: boolean | null; expiresAt?: number | null; reason?: 'closed' | 'timeout' }>;
  profiles(): Promise<{ supported: boolean; profiles: string[] }>;
  removeProfile(profile: string): Promise<{ removed: boolean }>;
}

export class AgenttyError extends Error {
  code: number;
}

export interface Plugin {
  info: InitializeParams | null;
  context: Context | null;
  command(id: string, handler: (call: { context: Context; args: Record<string, unknown> }) => unknown): Plugin;
  onEvent(elementId: string, handler: (event: UiEvent, context: Context) => unknown): Plugin;
  onAnyEvent(handler: (event: UiEvent, context: Context) => unknown): Plugin;
  onUrl(path: string, handler: (open: UrlOpen) => unknown): Plugin;
  onAnyUrl(handler: (open: UrlOpen) => unknown): Plugin;
  onActivate(handler: (info: InitializeParams) => unknown): Plugin;
  onPanelOpen(handler: (context: Context) => unknown): Plugin;
  onPanelClose(handler: (context: Context) => unknown): Plugin;
  onContextChange(handler: (context: Context) => unknown): Plugin;
  onShutdown(handler: () => unknown): Plugin;
  setPanel(tree: UiNode, options?: { instance?: string }): Promise<void>;
  instances(): Promise<{ instance: string; title: string | null; active: boolean }[]>;
  setInstanceTitle(instance: string, title: string): Promise<void>;
  onInstanceOpen(handler: (event: { instance: string; title: string | null }) => unknown): Plugin;
  onInstanceClose(handler: (event: { instance: string }) => unknown): Plugin;
  showPanel(): Promise<void>;
  notify(message: string, kind?: 'info' | 'success' | 'warning' | 'error'): Promise<void>;
  setBadge(text: string): Promise<void>;
  getContext(): Promise<Context>;
  injectPrompt(request: PromptRequest): Promise<{ status: 'sent' | 'asked'; paneId?: number }>;
  sendToTerminal(request: { paneId?: number; text: string; submit?: boolean }): Promise<{ paneId: number }>;
  getSession(request?: { paneId?: number; maxTurns?: number }): Promise<Session>;
  listWorkspaces(): Promise<WorkspaceInfo[]>;
  openUrl(url: string): Promise<void>;
  revealPath(path: string): Promise<void>;
  /** The in-app browser on the manifest's `browser.sites`. Needs `browser.control`. */
  browser: Browser;
  onBrowserHidden(handler: (event: { tabId: number }) => unknown): Plugin;
  log(...parts: unknown[]): void;
  start(): Plugin;
}

export function createPlugin(streams?: { input?: NodeJS.ReadableStream; output?: NodeJS.WritableStream }): Plugin;
