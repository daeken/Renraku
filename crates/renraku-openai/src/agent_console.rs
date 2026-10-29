//! Foreground presentation only. The managed journal and receipts own recovery.
use renraku_vm::agent::{AgentEvent, AgentEventKind, AgentLimits, AgentReport, AgentStop};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct AgentConsole {
    pub auth: PathBuf,
    pub universe: PathBuf,
    pub receipts: PathBuf,
    pub compiler_ledger: PathBuf,
    pub compiler_driver: PathBuf,
    pub profile: String,
    pub model: String,
}
fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn executable(name: &str) -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join(name)))
        .filter(|p| p.is_file())
        .map(|p| path(&p))
        .unwrap_or_else(|| name.into())
}
fn shell_argument(value: &str) -> String {
    // Always single-quote, including newlines, dollar signs, and backticks.
    // These are displayed commands, never passed to a host shell by Renraku.
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn shell_command(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|s| shell_argument(s))
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn event_line(event: &AgentEvent) -> String {
    let label = match event.kind {
        AgentEventKind::ActionSaved => "saved action",
        AgentEventKind::ResultSaved if event.ok == Some(false) => "saved error",
        AgentEventKind::ResultSaved => "saved result",
    };
    format!(
        "turn {} {label} [{}]: {}",
        event.turn, event.action, event.summary
    )
}
fn guidance(stop: &AgentStop) -> (&'static str, &'static str) {
    match stop {
        AgentStop::Finished(_) => (
            "finished",
            "Finished this goal. The result and conversation are saved. Use agent-inspect to check support for a follow-up via agent-input.",
        ),
        AgentStop::InstructionBudget => (
            "resume",
            "Paused at the guest instruction budget. Resume the same session with a positive instruction budget.",
        ),
        AgentStop::ProviderBudget => (
            "resume",
            "Paused at the model-operation budget. Resume the same session to continue; receipt lookups also use this budget.",
        ),
        AgentStop::ToolBudget => (
            "resume",
            "Paused at the tool-attempt budget. Resume the same session with a positive tool budget; saved mutation receipts prevent replaying a committed call.",
        ),
        AgentStop::TurnLimit => (
            "inspect",
            "The session's 32-turn limit is reached. Larger per-run budgets cannot extend it. Inspect the saved conversation before starting new work.",
        ),
        AgentStop::TranscriptLimit => (
            "inspect",
            "The session transcript limit is reached. Larger per-run budgets cannot extend it. Inspect the saved conversation before starting new work.",
        ),
        AgentStop::OutcomeUnknown(_) => (
            "reconcile_only",
            "A model outcome is uncertain. Resume only to check the same completion ledger for a receipt. A missing receipt stops again; the request is never resubmitted. Keep the original universe, ledger, and profile.",
        ),
    }
}
impl AgentConsole {
    fn resume_arguments(&self, limits: AgentLimits) -> Vec<String> {
        vec![
            executable("renraku-openai"),
            "--auth-dir".into(),
            path(&self.auth),
            "agent-drive".into(),
            path(&self.universe),
            path(&self.receipts),
            path(&self.compiler_ledger),
            path(&self.compiler_driver),
            self.profile.clone(),
            self.model.clone(),
            limits.instructions.max(10_000).to_string(),
            limits.provider_operations.max(1).to_string(),
            limits.tool_actions.max(16).to_string(),
            "--allow-model-calls".into(),
        ]
    }
    fn inspect_arguments(&self) -> Vec<String> {
        vec![
            executable("renraku-vm"),
            "agent-inspect".into(),
            path(&self.universe),
        ]
    }
    pub fn recovery(&self, stop: &AgentStop, limits: AgentLimits) -> Value {
        let (kind, message) = guidance(stop);
        let mut recovery =
            json!({"kind":kind,"message":message,"inspectArgs":self.inspect_arguments()});
        if matches!(kind, "resume" | "reconcile_only") {
            recovery["resumeArgs"] = json!(self.resume_arguments(limits));
        }
        recovery
    }
    pub fn report(&self, report: &AgentReport, limits: AgentLimits) -> Value {
        let mut result = report.summary();
        result["recovery"] = self.recovery(&report.stop, limits);
        result
    }
    pub fn show_guidance(&self, stop: &AgentStop, limits: AgentLimits) {
        let recovery = self.recovery(stop, limits);
        eprintln!("{}", recovery["message"].as_str().unwrap());
        if let Some(args) = recovery["resumeArgs"].as_array() {
            let args: Vec<_> = args
                .iter()
                .map(|a| a.as_str().unwrap().to_owned())
                .collect();
            eprintln!("Resume: {}", shell_command(&args));
        }
        eprintln!("Inspect: {}", shell_command(&self.inspect_arguments()));
    }
    pub fn show_failure(&self) {
        eprintln!(
            "Stopped with an error. Saved work is retained. Inspect the checkpoint and fix the reported cause, then use agent-drive with the same paths, profile, and model. For an uncertain model outcome, resume only reconciles the original completion ledger; it never resubmits."
        );
        eprintln!("Inspect: {}", shell_command(&self.inspect_arguments()));
        eprintln!(
            "Frames: {}",
            shell_command(&[
                executable("renraku-vm"),
                "inspect-checkpoint".into(),
                path(&self.universe)
            ])
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn console() -> AgentConsole {
        AgentConsole {
            auth: "/private/auth".into(),
            universe: "/tmp/world 'quoted' $x`cmd`\n.db".into(),
            receipts: "/private/receipts".into(),
            compiler_ledger: "/tmp/compiler.db".into(),
            compiler_driver: "/tmp/compiler.dll".into(),
            profile: "profile-id".into(),
            model: "chosen-model".into(),
        }
    }
    #[test]
    fn recovery_distinguishes_resumable_budgets_from_terminal_limits_and_uncertain_inference() {
        let limits = AgentLimits {
            instructions: 0,
            provider_operations: 0,
            tool_actions: 0,
        };
        for stop in [
            AgentStop::InstructionBudget,
            AgentStop::ProviderBudget,
            AgentStop::ToolBudget,
        ] {
            let recovery = console().recovery(&stop, limits);
            assert_eq!(recovery["kind"], "resume");
            let args = recovery["resumeArgs"].as_array().unwrap();
            assert_eq!(args[4], path(&console().universe));
            assert_eq!(&args[10..13], &[json!("10000"), json!("1"), json!("16")]);
            assert_eq!(args.last().unwrap(), "--allow-model-calls");
        }
        for stop in [
            AgentStop::TurnLimit,
            AgentStop::TranscriptLimit,
            AgentStop::Finished(json!({"ok":true})),
        ] {
            assert!(
                console()
                    .recovery(&stop, limits)
                    .get("resumeArgs")
                    .is_none()
            );
        }
        let unknown = console().recovery(&AgentStop::OutcomeUnknown("effect".into()), limits);
        assert_eq!(unknown["kind"], "reconcile_only");
        assert!(
            unknown["message"]
                .as_str()
                .unwrap()
                .contains("never resubmitted")
        );
        assert_eq!(unknown["resumeArgs"][5], "/private/receipts");
        assert_eq!(unknown["resumeArgs"][8], "profile-id");
    }
    #[test]
    fn displayed_resume_arguments_survive_shell_metacharacters_without_execution() {
        let limits = AgentLimits {
            instructions: 10_000,
            provider_operations: 1,
            tool_actions: 16,
        };
        let args = console().resume_arguments(limits);
        // Ask the shell to parse the display as positional arguments, then print
        // them. A value's substitutions must stay literal, including backticks.
        let displayed = shell_command(&args);
        let script = format!("set -- {displayed}\nprintf '%s\\0' \"$@\"");
        let result = std::process::Command::new("sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(result.status.success());
        let parsed: Vec<_> = result
            .stdout
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect();
        assert_eq!(parsed, args);
        assert!(result.stderr.is_empty());
    }
}
