# Roadmap

Where Agentty is going, and why. Written 2026-09-22 against v0.1.16.

This is a planning document, not a promise. Dates are targets, and an item that turns out to be
wrong is meant to be struck out here rather than quietly carried.

## 1. Positioning

**Agentty** is the app: a native workbench where several AI agents work at once and you watch what
they produce. Its existing copy stays as it is.

**AgenttyOS** is the layer above it: work described once as a sequence of steps, walked through the
agents by a plugin. It moves out of the Plugins page into its own entry in the activity bar.

| | Copy |
|---|---|
| AgenttyOS, headline | *Teach it once. It keeps working.* — KO: 한 번 가르치면, 계속 일하는 팀 |
| AgenttyOS, subline | Describe a job as steps and the agents walk them. Or take someone else's team and run it. |
| Vision | *One person, the output of a team.* — KO: 한 사람이 팀만큼 만들 수 있게 |

The axis that separates us from everything else is **supervised autonomy**: you hand work over and
you watch it happen. The opposite position — hand it over and don't watch — is a real position that
OpenClaw and Devin hold, which is what makes ours a position at all.

### What we do not say

- Not "anyone, in every field". Breadth is the documented cause of death for OpenAI's AgentKit
  (sunsetting 2026-11-30) and ChatGPT Atlas (discontinued 2026-08-09, ten months after launch).
- Not "the infinite possibilities of AI". A claim whose opposite is absurd carries no information.
- Not a second OpenClaw. It is MIT, run by a non-profit, sponsored by OpenAI, past 250k stars, with
  5,400+ skills on ClawHub and 29 chat channels. Meeting it on "anyone automates anything with
  shared skills, for free" is a fight with no path.

Where OpenClaw is a chat-driven assistant that acts unattended, Agentty is a window where work is
produced and checked: a diff, a running page in the browser, a row in the database, a cost per
session. OpenClaw structurally cannot show those, because a chat thread is not a workbench. Our
territory is the work that is expensive when it is wrong.

### Audience, in order

Developers now; makers (developers and content producers) once Phase 2 actually ships. Adjacent
expansion, never simultaneous. Scheduling and inbox triage belong to OpenClaw, and we should say so.

## 2. What the research changed

| Assumption we started with | What the market says | Consequence |
|---|---|---|
| A standalone AI browser is a differentiator | Atlas discontinued after ten months; Dia sold to Atlassian; Comet went from $200/mo to free | Idea 3 is not a browser. It is a repeatable browser-automation runtime |
| Routing a user's subscription traffic to a local model saves them money | Anthropic's terms forbid using subscription OAuth tokens in third-party products, and have been enforced. Qwen3-Coder is near-frontier on SWE-bench (77.2 vs 80.8) and far behind on real agentic tool calling (22.8 vs 89.4) | Idea 2 is a local sidecar for scoped side work, not a replacement for Claude Code |
| A first-party store is the end state | The GPT Store caps creators at $100–500/mo outside the top 0.01%; MCP registries industry-wide have no monetization layer yet; JetBrains' 15% is the only revenue share that demonstrably works | Idea 5 is correctly long-term, and the trust layer comes before payments |

## 3. The moat

Nobody else combines: native performance on all three desktop operating systems, **several agent
vendors as first-class panes** (Claude Code, Codex, Gemini), worktrees, a plugin runtime, a browser,
and a database panel — in one binary.

- Vendor-owned apps (Claude Code Desktop, the Codex app, Antigravity, Agent HQ) are single-vendor by
  construction and will never support a competitor's CLI deeply. Neutrality is a stance only we can
  credibly take.
- The Electron orchestrators (Conductor, Nimbalyst, Vibe Kanban) have no browser and no database,
  and degrade exactly where it matters: five to ten panes at once.
- Cloud orchestration alone is not defensible. Terragon shut down, Roo Code pivoted, AgentKit is
  being wound down — all in 2026, all after the labs shipped worktree-per-session themselves.

Because the app is GPL-3.0, binary features can be forked. **Revenue has to sit in services**, not
in features: hosted relays, a routed API, team sync. Phase 3.

## 4. Where the ideas land

| # | Idea | Verdict | Change |
|---|---|---|---|
| 1 | Plugin expansion | First, scope widened | Promote the plugin runtime to a workflow runtime. Triggers, schedules, secrets, background execution, audit |
| 2 | Jev integration and a local-LLM routing engine | Split in two | Jev is a decision layer, not a model to route to. The local LLM is an execution leg for scoped work |
| 3 | AI browser plugin | Redefined | No Chromium fork. CDP against a bundled or installed Chromium, with recording, auth profiles and headless scheduling |
| 4 | AgenttyOS workflows and sharing | Core differentiator | Local-first, no per-run metering, direct repo access — the thing Zapier and Gumloop cannot do |
| 5 | First-party marketplace | Long-term, correctly judged | Trust layer before commerce |
| 6 | Media generation plugins | Phase 2 | Local first, cloud fallback. Apache-2.0 models by default |
| 7 | Unified skill / agent / MCP marketplace | Before idea 5 | The whole industry is pre-commerce here; the standard is unclaimed |
| 8 | Session manager and sharing | Most undervalued item on the list | Three gaps nobody has filled, and we already have prototypes of two |
| 9 | Instant use of any external solution | Split in two | Manifest-aware now; arbitrary repos later |

### Notes that matter

**Idea 8.** The confirmed unfilled gaps are: handing a live session to another person with worktree
and diff intact; relaying a subtask from one agent vendor to another with structured context; and
attributing cost and output to a specific agent, session and model. Session Flow and the usage page
are the beginnings of the first two. No competitor has any of the three.

**Idea 6 constraints.** Midjourney has no official API and its terms forbid automated access — do
not build on it. The Sora 2 API is being shut down 2026-09-24. Default to Apache-2.0 models
(Qwen-Image, Wan 2.2, FLUX schnell); FLUX.1-dev output is usable commercially but reselling a
generation service on it needs a paid licence, so that needs an explicit acknowledgement in the UI.

**Idea 9 sandboxing.** Docker Desktop's shared-VM model on macOS had a container escape in September
2026 (CVE-2026-77179). For executing an unknown repository's code, prefer Apple's `container`
framework, which gives each container its own lightweight VM.

## 5. The decision layer (Jev / TypeSafe)

Jev is not a language model to route work to. It is a "System One" model: non-autoregressive, single
pass, returning **typed decisions with calibrated probabilities** — `Choice`, `Score`, `Noul` — in
70–500 ms at $0.042 per million input tokens, output free. One endpoint, `POST /v1/systemone`, takes
a state plus a named map of typed questions and evaluates them in parallel. Python and JS SDKs, and
an agent skill for Claude Code and Codex already exist.

It is the primitive missing underneath ideas 2, 3, 4 and 8:

| Where | Primitive | Why |
|---|---|---|
| AgenttyOS step checks (`has_headings`, `long_enough`) | Score / Noul | Hand-written predicates become calibrated judgements, and the approval gate becomes a confidence threshold. This is what makes workflows actually hold together |
| Model routing | Choice + confidence | Answers the failure RouterArena documented, where routers over-select the expensive model |
| Browser injection screening | Noul | Screening for injection attempts is a use case TypeSafe advertises. Claude for Chrome measured 23.6% unmitigated, 11.2% mitigated — this is the live risk |
| Database write approval | Score | Grade the risk of the statement the agent wants to run |
| Agent status in a pane | Choice | Fewer false readings on the app's most visible feature — but see the conflict below |
| Marketplace trust grade | Score | Manifest and permissions to a risk score |
| Conflict arbitration | Score | Rank the collision risk between two worktrees' diffs |

### The conflict, and how we resolve it

The README promises that transcripts and prompts never leave the machine. **Jev has closed weights
and is hosted-only** — no self-hosting, no local path, US hosting, zero data retention for
enterprise customers only, and it is in early access behind a waitlist as of its launch six days
before this document.

So it goes behind an interface:

1. A `Decision` abstraction in `agentty-bridge` (`choice` / `score` / `noul`) with swappable
   backends: `heuristic` (what we do today), `local`, `jev`.
2. **Jev off by default.** Turning it on states what leaves the machine, and what is sent is a
   structured, summarised state — never a raw transcript.
3. Never a hard dependency. Single vendor, closed weights, six days old.

The cheapest first contact is to surface TypeSafe's existing Claude Code / Codex agent skill on the
extensions page: near-zero integration cost for a real usage signal.

## 6. Phases

### Phase 0 — Promote the runtime (2026 Q4)

The shared foundation for ideas 4, 8 and 9. Nothing above this ships without it.

- **Workflow runtime.** Triggers (cron, file change, git event, `agentty://` webhook), background
  execution (`host/timer` only lets a plugin wait today), run history, retries, failure alerts.
- **Credential broker.** A plugin uses a secret without seeing it, injected via the keychain.
- **Cross-agent relay.** Formalise the handoff — today a Markdown document plus a prompt — into
  structured context transfer, so Claude can delegate a subtask to Codex.
- **Attribution v1.** Cost, tokens and output per session, agent and model.
- **Audit log.** Every autonomous action recorded, with a kill switch.
- **`Decision` abstraction** with an opt-in Jev backend. First uses: AgenttyOS step checks and
  database write risk.
- **AgenttyOS in the activity bar**, split out of the Plugins page (see section 7).

### Phase 1 — Automation that runs (2027 H1)

- **Browser automation plugin.** CDP-driven, over a bundled or installed Chromium; recorded and
  editable workflows; persistent authenticated profiles; headless background runs. The seven
  security rules apply from day one: page content is untrusted input, consent gates on high-risk
  actions, site allow/block lists, isolation from other extensions, credentials injected rather than
  shown to the model, no fighting anti-bot systems, full action audit.
- **Local LLM sidecar.** llama.cpp and MLX vendored directly, GGUF models. Routing limited to side
  work — commit messages, summaries, classification, naming, first-pass diff review. Savings shown
  from measured instrumentation, not vendor claims.
- **Instant external solutions v1.** Detect `devcontainer.json`, Cog or `AGENTS.md` and run it.
  Sandboxed with Apple's `container`.
- **Conflict arbitration v1.** Detect schema and migration collisions across worktrees — the class
  of conflict git cannot see, and the one our database panel can.

### Phase 2 — Ecosystem (2027 H2)

- **AgenttyOS packs**: marketing, SNS, content, development — each a marketplace workflow.
- **Media generation plugins**: local first with cloud fallback, Apache-2.0 models by default, a
  licence warning where the model requires it.
- **Unified skill / agent / MCP hub** (idea 7), designed as an execution-permission proxy rather
  than an installer.
- **Marketplace trust layer**: signing, provenance (the pattern n8n adopted in May 2026), permission
  grades, audit.
- Widen the audience line from "developers" to "makers" — only once the packs are actually shipping.

### Phase 3 — Commerce (2028+)

- A real store, with **developer-issued licence keys** rather than first-party payments. Figma's
  built-in commerce excludes payouts in India, Brazil, Nigeria and others; the VS Code and Chrome
  pattern — free discovery, developer-run Stripe or Paddle — is what developers already default to.
- Subscription or one-off plugin licences, **not metered credits**. Metering contradicts the
  local-first, nothing-per-run positioning.
- Team features: live session handoff, RBAC, organisation audit. This is the paid tier.
- A routed API tier, **API-key based only** — never proxying consumer subscription tokens.

## 7. Naming

`AgentOS` becomes **AgenttyOS** (pronounced 에이전티 오에스), and the grammar changes with it.

`AgentOS` is used today as a countable common noun — "an AgentOS is a plugin that…", "Blogger
AgentOS". `AgenttyOS` is a product name, so that reading breaks.

- AgenttyOS is the layer that runs work through Agentty's agents.
- A **workflow** is a plugin built on it.
- "Blogger AgentOS" becomes "Blogger — an AgenttyOS workflow".

Scope:

| Where | What |
|---|---|
| `docs/plugins/agentos.md` | Body, and the filename, to `agenttyos.md` |
| `docs/plugins/{README,protocol,usage}.md` | Four links and mentions |
| `crates/agentty-app/src/workbench/plugin_host.rs` | Two comments |
| `CHANGELOG.md` | The 0.1.15 entry is shipped history — leave it, record the rename under Unreleased |
| Agentty-Marketplace | The Rust SDK module `agentty_plugin::agentos` and the Blogger plugin's name. Another repository; the module rename breaks compatibility, so it goes with a version bump |
| agentty.run | Handled separately, on the website |

AgenttyOS is a product name and is **not** translated: the same Latin spelling in en, ko, ja and zh.

### Splitting AgenttyOS out of the Plugins page

Three decisions come with the split:

1. **The boundary.** A workflow is still a plugin, and the same thing appearing in two places is
   confusing. The manifest needs a classification field (`contributes.workflow`, or a category) so
   the AgenttyOS panel shows workflows and the Plugins page shows tools. One marketplace, split by
   tag.
2. **The empty state is the first impression.** Someone opening a new activity-bar entry must not
   find nothing: three suggested workflows and "build your own", reusing the Plugins page's existing
   *Build your own* flow.
3. **Carry the team metaphor into the UI.** Draw each workflow as a role card — blogger, marketer,
   SNS — and let a running card show that it is working, the same vocabulary the pane status already
   uses. The concept is then learned once, not twice.

## 8. Risks

| Risk | Response |
|---|---|
| Scope sprawl | "A total platform" is what killed AgentKit and Atlas. Adjacent expansion only, in the stated order |
| The labs absorbing the app layer | Stay where a single vendor cannot go: neutrality, native performance, the surrounding toolchain |
| GPL-3.0 makes features forkable | Revenue in services, Phase 3 |
| Prompt injection through the browser | Designed in from day one, not added later |
| Local models failing at tool calls | Narrow routing scope, automatic fallback, and make the fallback visible to the user |
| Jev is six days old, closed-weight, single-vendor, hosted-only | Behind the `Decision` interface, off by default, never a hard dependency |
| Promising "makers" before Phase 2 ships | Keep the audience line at developers until the packs exist |

## 9. Open decisions

1. **Phase 0 order** — triggers and scheduler first, or attribution first? The former unlocks ideas
   4, 8 and 9; the latter builds the evidence for idea 2. Recommendation: triggers first.
2. **Browser packaging** — a separately downloaded plugin, or an extension of the existing in-app
   browser? CDP removes much of the need for a bundled Chromium, which weakens the "it is large, so
   it is a plugin" premise.
3. **When the audience line widens** from developers to makers. Tied to Phase 2 actually shipping.

## 10. Evidence

Competitive landscape: [Conductor](https://conductor.build/) · [Nimbalyst](https://nimbalyst.com/) ·
[Vibe Kanban](https://vibekanban.com/) · [Zed parallel agents](https://zed.dev/blog/parallel-agents) ·
[Copilot fleet](https://github.blog/ai-and-ml/github-copilot/run-multiple-agents-at-once-with-fleet-in-copilot-cli/) ·
[OpenClaw](https://openclaw.ai/) · [OpenClaw skills](https://docs.openclaw.ai/tools/skills)

Routing and local models: [Arch-Router](https://arxiv.org/html/2506.16655v1) ·
[Qwen3-Coder as an agent backend](https://dev.to/sikamikanikobg/local-llm-vs-claude-benchmarking-qwen3-coder30b-as-a-production-agent-backend-482b) ·
[Jan](https://www.jan.ai/docs/desktop/local-engine/llama-cpp)

Decision layer: [TypeSafe AI](https://typesafe.ai/) ·
[System One models](https://typesafe.ai/blog/introducing-system-one-models-and-jev) ·
[docs](https://docs.typesafe.ai/)

Browsers: [Stagehand v3](https://www.browserbase.com/blog/stagehand-v3) ·
[Browser Use](https://github.com/browser-use/browser-use) ·
[Claude in Chrome threat analysis](https://labs.zenity.io/post/claude-in-chrome-a-threat-analysis)

Workflow platforms and marketplaces: [AgentKit wind-down](https://montanalabs.ai/news/openai-s-agentkit-and-the-eight-month-lifespan-of-agent-builder/) ·
[JetBrains revenue sharing](https://plugins.jetbrains.com/docs/marketplace/revenue-sharing-and-fees.html) ·
[n8n node verification](https://docs.n8n.io/integrations/creating-nodes/build/reference/verification-guidelines/) ·
[GPT Store monetization](https://www.wildnetedge.com/blogs/chatgpt-app-monetization)

Media and sandboxing: [Cog](https://github.com/replicate/cog) ·
[Dev Container spec](https://containers.dev/implementors/spec/) ·
[Docker sandbox escape, Sept 2026](https://thehackernews.com/2026/09/critical-docker-sandboxes-flaw-lets.html)
