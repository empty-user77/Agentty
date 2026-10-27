//! The to-do list a Claude Code session keeps for itself, followed from the hooks its pane already
//! receives: `TodoWrite` hands over the whole list each time, newer versions add items with
//! `TaskCreate` and tick them with `TaskUpdate`. Shown on the workspace card as "4/9".

pub use agentty_bridge::subagent_trace::{PlanItem, PlanStatus};

/// A change to the list, from one `PostToolUse` hook.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanEvent {
    Replace(Vec<PlanItem>),
    Create { id: Option<String>, text: String },
    Update { id: String, status: Option<PlanStatus>, text: Option<String>, deleted: bool },
}

fn status(value: Option<&str>) -> PlanStatus {
    match value {
        Some("completed") => PlanStatus::Done,
        Some("in_progress") => PlanStatus::Active,
        _ => PlanStatus::Pending,
    }
}

impl PlanEvent {
    /// The plan change in a hook payload, if it is one. Only after the tool ran: a created task
    /// has its id then.
    pub fn from_hook(event: Option<&str>, tool: Option<&str>, payload: &serde_json::Value) -> Option<Self> {
        if event != Some("PostToolUse") {
            return None;
        }
        let input = &payload["tool_input"];
        let text = |v: &serde_json::Value| v.as_str().map(|s| s.chars().take(200).collect::<String>());
        match tool? {
            "TodoWrite" => Some(Self::Replace(
                input["todos"]
                    .as_array()?
                    .iter()
                    .take(100)
                    .filter_map(|todo| Some(PlanItem { id: None, text: text(&todo["content"])?, status: status(todo["status"].as_str()) }))
                    .collect(),
            )),
            "TaskCreate" => {
                let response = &payload["tool_response"];
                let id = response["task"]["id"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| response["task"]["id"].as_u64().map(|n| n.to_string()))
                    .or_else(|| {
                        // "Task #7 created …"
                        let said = response.as_str()?;
                        said.split('#').nth(1)?.split(|c: char| !c.is_ascii_digit()).next().filter(|n| !n.is_empty()).map(str::to_string)
                    });
                Some(Self::Create { id, text: text(&input["subject"])? })
            }
            "TaskUpdate" => {
                let id = input["taskId"].as_str().map(str::to_string).or_else(|| input["taskId"].as_u64().map(|n| n.to_string()))?;
                let deleted = input["status"] == "deleted";
                Some(Self::Update {
                    id,
                    status: input["status"].as_str().filter(|_| !deleted).map(|s| status(Some(s))),
                    text: text(&input["subject"]),
                    deleted,
                })
            }
            _ => None,
        }
    }
}

pub fn apply(plan: &mut Vec<PlanItem>, event: PlanEvent) {
    match event {
        PlanEvent::Replace(items) => *plan = items,
        PlanEvent::Create { id, text } => {
            if plan.len() < 100 {
                plan.push(PlanItem { id, text, status: PlanStatus::Pending });
            }
        }
        PlanEvent::Update { id, status, text, deleted } => {
            if deleted {
                plan.retain(|item| item.id.as_deref() != Some(id.as_str()));
                return;
            }
            if let Some(item) = plan.iter_mut().find(|item| item.id.as_deref() == Some(id.as_str())) {
                if let Some(status) = status {
                    item.status = status;
                }
                if let Some(text) = text {
                    item.text = text;
                }
            }
        }
    }
}

/// (done, all) items.
pub fn progress(plan: &[PlanItem]) -> (usize, usize) {
    (plan.iter().filter(|p| p.status == PlanStatus::Done).count(), plan.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(tool: &str, json: &str) -> Option<PlanEvent> {
        PlanEvent::from_hook(Some("PostToolUse"), Some(tool), &serde_json::from_str(json).unwrap())
    }

    #[test]
    fn a_plan_follows_its_tools() {
        let mut plan = Vec::new();
        let write = post(
            "TodoWrite",
            r#"{"tool_input":{"todos":[{"content":"Read the spec","status":"completed"},{"content":"Write tests","status":"in_progress"},{"content":"Ship","status":"pending"}]}}"#,
        )
        .unwrap();
        apply(&mut plan, write);
        assert_eq!(progress(&plan), (1, 3));
        assert_eq!(plan[1].status, PlanStatus::Active);

        let mut tasks = Vec::new();
        apply(&mut tasks, post("TaskCreate", r#"{"tool_input":{"subject":"Scaffold"},"tool_response":{"task":{"id":"7"}}}"#).unwrap());
        apply(
            &mut tasks,
            post("TaskCreate", r#"{"tool_input":{"subject":"Docs"},"tool_response":"Task #8 created successfully"}"#).unwrap(),
        );
        apply(&mut tasks, post("TaskUpdate", r#"{"tool_input":{"taskId":"7","status":"completed"}}"#).unwrap());
        assert_eq!(progress(&tasks), (1, 2));
        apply(&mut tasks, post("TaskUpdate", r#"{"tool_input":{"taskId":"8","status":"deleted"}}"#).unwrap());
        assert_eq!(tasks.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["Scaffold"]);

        // Before the tool ran, or another tool: no change.
        let pre = serde_json::json!({"tool_input": {"todos": []}});
        assert_eq!(PlanEvent::from_hook(Some("PreToolUse"), Some("TodoWrite"), &pre), None);
        assert_eq!(post("Bash", r#"{"tool_input":{"command":"ls"}}"#), None);
    }
}
