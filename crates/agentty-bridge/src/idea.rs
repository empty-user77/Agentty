//! "Build my idea": turns a few chat messages and attached documents into a new project folder
//! and the first prompt for the agent that builds a demo of it.
//!
//! The folder gets `docs/idea/IDEA.md` (the messages), `docs/idea/attachments/` (copies of the
//! attached files), `docs/idea/BUILD_GUIDE.md` (how to work: plan, parallel subagents, live preview
//! in Agentty's browser, Vercel-ready) and `.claude/settings.json` so Claude Code can install
//! packages and run the dev server without asking about every command. Everything else is left
//! empty so project scaffolders (`create-next-app`, `create vite`) still accept the folder.

use crate::fsutil;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Attachments larger than this are skipped (the agent could not read them anyway).
pub const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_ATTACHMENTS: usize = 20;

/// Where new idea projects go by default: `~/AgenttyProjects` (no spaces: some dev tools choke on them).
pub fn default_root() -> PathBuf {
    fsutil::home().join("AgenttyProjects")
}

/// What the user wrote on the idea page.
#[derive(Debug, Clone, Default)]
pub struct IdeaInput {
    /// Chat messages and pasted documents, in order.
    pub messages: Vec<String>,
    pub attachments: Vec<PathBuf>,
    /// Project name typed by the user; derived from the first message when empty.
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IdeaProject {
    pub dir: PathBuf,
    pub title: String,
    /// First message for the agent.
    pub prompt: String,
    /// Attachments that were not copied (missing, a folder, or too large).
    pub skipped: Vec<PathBuf>,
}

/// A short title from the name or the first line of the first message.
pub fn project_title(input: &IdeaInput) -> String {
    let source = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .or_else(|| input.messages.iter().flat_map(|m| m.lines()).map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("My idea");
    let cleaned: String = source.trim_start_matches(['#', '-', '*', ' ']).chars().filter(|c| !c.is_control()).collect();
    let mut title: String = cleaned.chars().take(40).collect();
    if cleaned.chars().count() > 40 {
        title = title.trim_end().to_string() + "…";
    }
    if title.trim().is_empty() {
        "My idea".into()
    } else {
        title
    }
}

/// Folder name: lowercase ASCII words joined by dashes; `fallback` when nothing ASCII is left
/// (e.g. a Korean-only title).
pub fn slug(title: &str, fallback: &str) -> String {
    let words: Vec<String> =
        title.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect();
    let mut slug = String::new();
    for word in words {
        if slug.len() + word.len() + 1 > 40 {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

/// `root/name`, or `root/name-2`, `-3`, … when taken.
pub fn unique_dir(root: &Path, name: &str) -> PathBuf {
    let first = root.join(name);
    if !first.exists() {
        return first;
    }
    (2..).map(|n| root.join(format!("{name}-{n}"))).find(|p| !p.exists()).expect("a free name")
}

/// Creates the project folder and returns the prompt that starts the build.
/// `language` is the UI language code (`en`, `ko`, `ja`, `zh`); `stamp` names untitled projects.
pub fn create_project(root: &Path, input: &IdeaInput, language: &str, stamp: &str) -> Result<IdeaProject> {
    let messages: Vec<&str> = input.messages.iter().map(|m| m.trim()).filter(|m| !m.is_empty()).collect();
    if messages.is_empty() && input.attachments.is_empty() {
        bail!("describe the idea or attach a document first");
    }
    let title = project_title(input);
    let dir = unique_dir(root, &slug(&title, &format!("idea-{stamp}")));
    let idea_dir = dir.join("docs").join("idea");
    std::fs::create_dir_all(&idea_dir).with_context(|| format!("could not create {}", idea_dir.display()))?;

    let (copied, skipped) = copy_attachments(&input.attachments, &idea_dir.join("attachments"))?;
    std::fs::write(idea_dir.join("IDEA.md"), idea_markdown(&title, &messages, &copied))?;
    std::fs::write(idea_dir.join("BUILD_GUIDE.md"), BUILD_GUIDE)?;
    let claude_dir = dir.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;
    std::fs::write(claude_dir.join("settings.json"), CLAUDE_SETTINGS)?;

    Ok(IdeaProject { prompt: build_prompt(language, &title), dir, title, skipped })
}

fn copy_attachments(files: &[PathBuf], into: &Path) -> Result<(Vec<String>, Vec<PathBuf>)> {
    let mut copied: Vec<String> = Vec::new();
    let mut skipped = Vec::new();
    for file in files {
        let fits = std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.len() <= MAX_ATTACHMENT_BYTES);
        let Some(name) = file.file_name().and_then(|n| n.to_str()).filter(|_| fits && copied.len() < MAX_ATTACHMENTS) else {
            skipped.push(file.clone());
            continue;
        };
        std::fs::create_dir_all(into)?;
        // Two files with the same name: keep both.
        let mut target = name.to_string();
        let mut n = 2;
        while copied.contains(&target) {
            let path = Path::new(name);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
            target = match path.extension().and_then(|e| e.to_str()) {
                Some(ext) => format!("{stem}-{n}.{ext}"),
                None => format!("{stem}-{n}"),
            };
            n += 1;
        }
        std::fs::copy(file, into.join(&target)).with_context(|| format!("could not copy {}", file.display()))?;
        copied.push(target);
    }
    Ok((copied, skipped))
}

fn idea_markdown(title: &str, messages: &[&str], attachments: &[String]) -> String {
    let mut out = format!("# {title}\n\nWritten by the product owner in Agentty's \"Build my idea\".\n\n## The idea\n\n");
    for message in messages {
        out.push_str(message);
        out.push_str("\n\n");
    }
    if !attachments.is_empty() {
        out.push_str("## Attached documents\n\nRead every one of these before planning (`docs/idea/attachments/`):\n\n");
        for name in attachments {
            out.push_str(&format!("- [{name}](attachments/{})\n", name.replace(' ', "%20")));
        }
    }
    out
}

/// The first message to the agent, in the user's language.
pub fn build_prompt(language: &str, title: &str) -> String {
    let template = match language {
        "ko" => PROMPT_KO,
        _ => PROMPT_EN,
    };
    let reply_in = match language {
        "ko" => "Korean",
        "ja" => "Japanese",
        "zh" => "Simplified Chinese",
        _ => "English",
    };
    template.replace("{title}", title).replace("{language}", reply_in)
}

const PROMPT_EN: &str = r#"Build a working demo of my idea "{title}". I'm not a developer, so take the lead as the senior engineer and product designer.

1. Read docs/idea/IDEA.md, everything in docs/idea/attachments/, and follow docs/idea/BUILD_GUIDE.md.
2. Don't ask me questions unless something essential is impossible to guess — choose sensible defaults and write your assumptions in PLAN.md.
3. Show me progress visually: get a first screen running early and open it in Agentty's in-app browser (browser_open), then keep refreshing it as you build.
4. Split independent parts across parallel subagents where your tools allow it, and integrate/review their work yourself.
5. When the demo works, tell me in plain words what you built, how to try it, and that I can publish it with the 🚀 Launch button.

Talk to me in {language}."#;

const PROMPT_KO: &str = r#"제 아이디어 "{title}"를 실제로 동작하는 데모로 만들어 주세요. 저는 개발자가 아니니 시니어 개발자이자 프로덕트 디자이너로서 주도적으로 진행해 주세요.

1. docs/idea/IDEA.md 와 docs/idea/attachments/ 의 모든 문서를 읽고, docs/idea/BUILD_GUIDE.md 의 방식대로 작업해 주세요.
2. 도저히 추측할 수 없는 핵심 사항이 아니면 질문하지 말고 합리적인 기본값으로 정한 뒤, 가정한 내용을 PLAN.md 에 적어 주세요.
3. 진행 상황을 눈으로 볼 수 있게 해 주세요: 첫 화면을 빨리 띄워서 Agentty 내장 브라우저(browser_open)로 열고, 만드는 동안 계속 새로고침해 주세요.
4. 서로 독립적인 부분은 가능하면 여러 서브에이전트에게 병렬로 나눠 맡기고, 결과를 직접 통합·검토해 주세요.
5. 데모가 완성되면 무엇을 만들었는지, 어떻게 써 보면 되는지 쉬운 말로 알려 주고, 🚀 출시 버튼으로 인터넷에 공개할 수 있다고 안내해 주세요.

저와는 한국어로 대화해 주세요."#;

/// How the agent should work. Kept in the project so later sessions can re-read it.
pub const BUILD_GUIDE: &str = r#"# Build guide (Agentty "Build my idea")

The owner of this project is not a developer. Your job: turn docs/idea/IDEA.md (and the attachments)
into a working demo they can see and click, ready to publish on Vercel.

## 1. Plan (short)
- Write PLAN.md: one-paragraph product summary, target user, the pages/screens and features of the
  demo, the tech stack, assumptions you made, and a task checklist (`- [ ]`).
- Scope for a demo: the core flow working end to end with realistic sample data beats many
  half-finished features.

## 2. Stack (unless the idea clearly needs something else)
- Web app: Next.js (App Router) + TypeScript + Tailwind CSS. Simple landing page: the same, or
  Vite + React. Must build with `npm run build` and deploy to Vercel with zero configuration.
- Data: start with local sample data / in-memory or localStorage. If the product needs a real
  database or login, design the code so Supabase can be plugged in later (a small data-access
  module), and note it in PLAN.md.
- When Supabase is used: the 🚀 Launch button creates or picks the hosted project and writes its
  URL and public key to `.env.local` — read exactly `NEXT_PUBLIC_SUPABASE_URL` and
  `NEXT_PUBLIC_SUPABASE_ANON_KEY` (`VITE_…` with Vite) and list both names in `.env.example`. Put
  the schema in `supabase/migrations/<timestamp>_<name>.sql` (Launch applies them), enable row
  level security on every table with policies that fit the app, and never use or ask for the
  service_role key.
- This folder already contains `docs/` and `.claude/`. If a scaffolder refuses a non-empty folder,
  scaffold into a temporary subfolder and move the files up.
- Use npm. Never commit secrets: put keys in `.env.local` (gitignored) and document names in
  `.env.example`.

## 3. Show progress live
- Get a first screen running early: start the dev server in the background (e.g.
  `npm run dev -- --port 3000`, or the next free port) and open it in Agentty's in-app browser with
  the `browser_open` tool. Reload it (`browser_go` reload) after meaningful changes so the owner
  watches the product take shape.
- If the browser tools are not available, print the local URL clearly instead.

## 4. Orchestrate
- Split independent parts (e.g. separate pages, components, data layer, styling) and hand them to
  parallel subagents (Task tool) with clear file ownership and acceptance criteria. Keep shared
  pieces (layout, design tokens, types) with you and create them first.
- Review and integrate subagent work yourself; keep PLAN.md's checklist up to date.

## 5. Quality bar
- Polished, modern, responsive UI (mobile and desktop), real copy instead of lorem ipsum, empty
  and error states, accessible contrast and labels.
- Verify before calling it done: `npm run build` passes, the page has no console errors
  (`browser_console`), and a screenshot (`browser_screenshot`) looks right.

## 6. Hand-off
- Write a short README.md (what it is, how to run it) and CLAUDE.md + AGENTS.md with the project
  conventions for future sessions.
- Tell the owner in plain words: what was built, how to try it, what could come next, and that the
  🚀 Launch button (Agentty) publishes it to the internet (GitHub + Vercel).
"#;

/// Lets Claude Code edit files and run the usual build commands in this new project without a
/// prompt for each one. Deleting files, network tools other than npm and git pushes still ask.
pub const CLAUDE_SETTINGS: &str = r#"{
  "permissions": {
    "defaultMode": "acceptEdits",
    "allow": [
      "Bash(npm:*)",
      "Bash(npx:*)",
      "Bash(node:*)",
      "Bash(mkdir:*)",
      "Bash(ls:*)",
      "Bash(cat:*)",
      "Bash(mv:*)",
      "Bash(cp:*)",
      "Bash(lsof -i:*)",
      "Bash(git init:*)",
      "Bash(git status:*)",
      "Bash(git add:*)",
      "Bash(git commit:*)",
      "mcp__agentty-browser"
    ]
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("agentty-idea-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn titles_come_from_the_name_or_first_line() {
        let input = IdeaInput { messages: vec!["\n# Dog walking marketplace\nwith booking".into()], ..Default::default() };
        assert_eq!(project_title(&input), "Dog walking marketplace");
        let named = IdeaInput { name: Some("  Petly ".into()), ..input };
        assert_eq!(project_title(&named), "Petly");
        let long = IdeaInput { messages: vec!["a".repeat(80)], ..Default::default() };
        assert!(project_title(&long).ends_with('…'));
        assert_eq!(project_title(&IdeaInput::default()), "My idea");
    }

    #[test]
    fn slugs_are_ascii_with_a_fallback() {
        assert_eq!(slug("Dog walking: Marketplace!", "x"), "dog-walking-marketplace");
        assert_eq!(slug("강아지 산책 앱", "idea-20260918-1200"), "idea-20260918-1200");
        assert_eq!(slug("카페 Cafe 2.0", "x"), "cafe-2-0");
        assert!(slug(&"word ".repeat(30), "x").len() <= 40);
    }

    #[test]
    fn creates_the_project_folder() {
        let root = temp_root("create");
        let doc = root.join("plan.md");
        std::fs::write(&doc, "# Spec\nfeatures").unwrap();
        let input = IdeaInput {
            messages: vec!["Recipe sharing site".into(), "  ".into(), "Users can save favorites".into()],
            attachments: vec![doc.clone(), doc.clone(), root.join("missing.pdf"), root.clone()],
            name: None,
        };
        let project = create_project(&root, &input, "ko", "20260918-1200").unwrap();
        assert_eq!(project.dir, root.join("recipe-sharing-site"));
        assert_eq!(project.title, "Recipe sharing site");
        assert_eq!(project.skipped, vec![root.join("missing.pdf"), root.clone()]);
        let idea = std::fs::read_to_string(project.dir.join("docs/idea/IDEA.md")).unwrap();
        assert!(idea.contains("Users can save favorites"));
        assert!(idea.contains("attachments/plan.md") && idea.contains("attachments/plan-2.md"));
        assert!(project.dir.join("docs/idea/attachments/plan-2.md").exists());
        assert!(project.dir.join("docs/idea/BUILD_GUIDE.md").exists());
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(project.dir.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(settings["permissions"]["defaultMode"], "acceptEdits");
        assert!(project.prompt.contains("Recipe sharing site") && project.prompt.contains("한국어로"));
        // A second project with the same title gets its own folder.
        let again = create_project(&root, &input, "en", "20260918-1201").unwrap();
        assert_eq!(again.dir, root.join("recipe-sharing-site-2"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_an_empty_idea() {
        let root = temp_root("empty");
        let input = IdeaInput { messages: vec!["   ".into()], ..Default::default() };
        assert!(create_project(&root, &input, "en", "x").is_err());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prompts_follow_the_language() {
        assert!(build_prompt("en", "Todo").contains("in English"));
        assert!(build_prompt("ja", "Todo").contains("Japanese"));
        assert!(build_prompt("ko", "할 일").contains("\"할 일\""));
        assert!(build_prompt("ko", "할 일").contains("한국어로") && !build_prompt("ko", "x").contains("Korean"));
    }
}
