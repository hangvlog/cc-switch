use super::*;
use std::{fs, os::unix::fs::PermissionsExt, time::{Duration, Instant}};

const CLI_ID: &str = "747a8e82-3bed-4378-be99-80f2dcec6d9b";
const LOCAL_ID: &str = "local_441d08c3-82d5-489d-a832-3f2fe95153be";

/// Stand-in for the CLI: asks for one permission, reports the decision.
const FAKE_CLI: &str = r#"#!/bin/sh
read first
case "$first" in *'"type":"user"'*) ;; *) exit 3;; esac
echo '{"type":"system","subtype":"init","session_id":"S"}'
echo '{"type":"control_request","request_id":"perm-1","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"/tmp/x"},"tool_use_id":"t1"}}'
read answer
case "$answer" in *'"allow"'*) d=ALLOWED;; *) d=DENIED;; esac
echo "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parent_tool_use_id\":null,\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"$d $ANTHROPIC_BASE_URL $CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST\"}]}}"
echo '{"type":"result","is_error":false,"duration_ms":5,"total_cost_usd":0,"result":"ok"}'
"#;

struct Fixture {
    root: tempfile::TempDir,
    claude: Claude,
    cwd: String,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.to_string_lossy().into_owned();
    let claude_home = root.path().join("claude-home");
    let project = claude_home.join("projects").join(index::encode_project(&cwd));
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join(format!("{CLI_ID}.jsonl")), [
        json!({"type":"user","uuid":"u1","parentUuid":null,"timestamp":"2026-10-30T08:37:17.130Z","message":{"role":"user","content":"hello"}}),
        json!({"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-10-30T08:37:18.130Z","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}),
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
    let app = root.path().join("Claude-3p");
    let index_dir = app.join("claude-code-sessions/acct/org");
    fs::create_dir_all(&index_dir).unwrap();
    fs::write(index_dir.join(format!("{LOCAL_ID}.json")), json!({"sessionId":LOCAL_ID,"cliSessionId":CLI_ID,"cwd":cwd,
        "title":"测试会话","createdAt":1_793_347_303_182u64,"lastActivityAt":1_793_347_415_589u64,"isArchived":false,
        "model":"claude-fable-5","permissionMode":"bypassPermissions"}).to_string()).unwrap();
    let library = app.join("configLibrary");
    fs::create_dir_all(&library).unwrap();
    let profile = "00000000-0000-4000-8000-000000157210";
    fs::write(library.join("_meta.json"), json!({"appliedId":profile}).to_string()).unwrap();
    fs::write(library.join(format!("{profile}.json")), json!({"inferenceProvider":"gateway",
        "inferenceGatewayBaseUrl":"http://127.0.0.1:9/claude-desktop","inferenceGatewayApiKey":"test-token",
        "inferenceModels":[{"name":"claude-fable-5"}]}).to_string()).unwrap();
    let cli = root.path().join("fake-claude");
    fs::write(&cli, FAKE_CLI).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let claude = Claude::with_paths(Paths {
        claude_home, app_data: Some(app), registry: root.path().join("storage/claude-sessions.json"), cli_override: Some(cli),
    });
    Fixture { root, claude, cwd }
}

fn wait_for(events: &ClaudeEvents, seen: &mut Vec<Value>, kind: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        seen.extend(events.drain());
        if let Some(found) = seen.iter().find(|e| e["params"]["kind"] == kind) {
            return found.clone();
        }
        assert!(Instant::now() < deadline, "no {kind} event in {seen:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn lists_and_reads_desktop_sessions() {
    let f = fixture();
    assert_eq!(f.claude.capability()["available"], true);
    let list = f.claude.dispatch("claude/thread/list", &json!({})).unwrap();
    assert_eq!(list["data"][0]["id"], LOCAL_ID);
    assert_eq!(list["data"][0]["desktop"]["canSend"], true);
    assert_eq!(list["projects"][0], f.cwd);
    let read = f.claude.dispatch("claude/thread/read", &json!({"threadId":LOCAL_ID})).unwrap();
    assert_eq!(read["thread"]["turns"][0]["items"][1]["text"], "hi");
    assert!(f.claude.dispatch("claude/thread/read", &json!({"threadId":"../etc"})).is_err());
}

#[test]
fn phone_turn_round_trips_an_approval() {
    let f = fixture();
    let events = f.claude.events();
    let request = uuid::Uuid::new_v4().to_string();
    let receipt = f.claude.dispatch("claude/turn/start", &json!({"threadId":LOCAL_ID,"requestId":request,"text":"go"})).unwrap();
    assert_eq!(f.claude.dispatch("claude/turn/start", &json!({"threadId":LOCAL_ID,"requestId":request,"text":"go"})).unwrap(), receipt);
    let mut seen = Vec::new();
    let approval = wait_for(&events, &mut seen, "approval");
    assert_eq!(approval["params"]["request"]["tool"], "Write");
    let read = f.claude.dispatch("claude/thread/read", &json!({"threadId":LOCAL_ID})).unwrap();
    assert_eq!(read["thread"]["desktop"]["approvals"][0]["requestId"], "perm-1");
    assert_eq!(read["thread"]["desktop"]["canSend"], false);
    f.claude.dispatch("claude/request/respond", &json!({"threadId":LOCAL_ID,"requestId":"perm-1","decision":"accept"})).unwrap();
    let done = wait_for(&events, &mut seen, "completed");
    assert_eq!(done["params"]["status"], "completed");
    let reply = seen.iter().find(|e| e["params"]["kind"] == "update").unwrap().to_string();
    assert!(reply.contains("ALLOWED http://127.0.0.1:9/claude-desktop 1"), "{reply}");
    let _ = &f.root;
}

#[test]
fn refuses_sessions_open_on_the_desktop_and_unknown_projects() {
    let f = fixture();
    let sessions = f.root.path().join("claude-home/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(sessions.join("1.json"), json!({"pid":std::process::id(),"sessionId":CLI_ID,"status":"idle"}).to_string()).unwrap();
    let error = f.claude.dispatch("claude/turn/start", &json!({"threadId":LOCAL_ID,"requestId":uuid::Uuid::new_v4().to_string(),"text":"go"})).unwrap_err();
    assert!(error.contains("电脑上打开"));
    let list = f.claude.dispatch("claude/thread/list", &json!({})).unwrap();
    assert_eq!(list["data"][0]["desktop"]["status"], "desktop-open");
    let create = f.claude.dispatch("claude/thread/create", &json!({"cwd":"/","requestId":uuid::Uuid::new_v4().to_string(),"text":"x"}));
    assert!(create.unwrap_err().contains("已有 Claude 会话"));
}
