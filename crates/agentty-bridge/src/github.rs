//! The pull request of a branch, read through the GitHub CLI the user has already signed in to.
//!
//! Nothing here authenticates or stores a token: it shells out to `gh`, which keeps its own
//! credentials. A repository without `gh`, without a GitHub remote or without a pull request
//! simply has none, and the UI shows nothing.

use crate::process;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    /// `OPEN`, `MERGED` or `CLOSED`, as GitHub reports it.
    pub state: String,
    pub title: String,
    pub url: String,
}

impl PullRequest {
    pub fn is_open(&self) -> bool {
        self.state.eq_ignore_ascii_case("OPEN")
    }

    pub fn is_merged(&self) -> bool {
        self.state.eq_ignore_ascii_case("MERGED")
    }
}

/// The pull request GitHub has for `branch` in `repo`, if any.
pub fn pull_request(repo: &Path, branch: &str) -> Option<PullRequest> {
    // A ref may be named `--web` or `--repo=someone/else`, and it would be read as a flag rather
    // than as the branch to ask about. `git::checkout` guards the same way before running `git`.
    if branch.is_empty() || branch.starts_with('-') || !repo.is_dir() {
        return None;
    }
    let output = process::command("gh")
        .current_dir(repo)
        .args(["pr", "view", branch, "--json", "number,state,title,url"])
        // Never wait on a login prompt, and keep the CLI from paging.
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse(&String::from_utf8_lossy(&output.stdout))
}

fn parse(json: &str) -> Option<PullRequest> {
    serde_json::from_str(json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_gh_prints() {
        let json = r#"{"number":262,"state":"OPEN","title":"Add the thing","url":"https://github.com/o/r/pull/262"}"#;
        let pr = parse(json).unwrap();
        assert_eq!(pr.number, 262);
        assert!(pr.is_open());
        assert!(!pr.is_merged());
        assert_eq!(pr.url, "https://github.com/o/r/pull/262");
        assert!(parse("no pull requests found").is_none());
    }

    #[test]
    fn a_branch_without_a_repository_has_no_pull_request() {
        assert!(pull_request(Path::new("/definitely/not/a/repo"), "main").is_none());
        assert!(pull_request(Path::new("."), "").is_none());
    }

    /// A branch out of a repository someone else wrote is not allowed to turn into a flag.
    #[test]
    fn a_branch_that_looks_like_a_flag_is_refused() {
        for branch in ["--web", "--repo=someone/else", "-R"] {
            assert!(pull_request(Path::new("."), branch).is_none(), "{branch} must not reach gh");
        }
    }
}
