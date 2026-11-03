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
    assert!(error.contains("不支持接收手机消息"), "{error}");
    let list = f.claude.dispatch("claude/thread/list", &json!({})).unwrap();
    assert_eq!(list["data"][0]["desktop"]["status"], "desktop-open");
    let create = f.claude.dispatch("claude/thread/create", &json!({"cwd":"/","requestId":uuid::Uuid::new_v4().to_string(),"text":"x"}));
    assert!(create.unwrap_err().contains("已有 Claude 会话"));
}

#[test]
fn mirrors_desktop_sidebar_lineage_and_activity() {
    let f = fixture();
    let app = f.root.path().join("Claude-3p");
    let fork_cli = "5f5c1525-2925-4ac6-bab8-a6236e198323";
    let fork_id = "local_17cbf74d-d1ca-4546-afc8-7ccc6fbebfee";
    let project = f.root.path().join("claude-home/projects").join(index::encode_project(&f.cwd));
    fs::write(project.join(format!("{fork_cli}.jsonl")), format!("{}\n{}\n",
        json!({"type":"user","uuid":"u1","parentUuid":null,"timestamp":"2026-11-02T03:00:00.000Z","message":{"role":"user","content":"fork"}}),
        json!({"type":"last-prompt","lastPrompt":"fork"}))).unwrap();
    fs::write(app.join(format!("claude-code-sessions/acct/org/{fork_id}.json")), json!({"sessionId":fork_id,"cliSessionId":fork_cli,
        "cwd":f.cwd,"title":"测试会话 (fork)","createdAt":1u64,"lastActivityAt":2u64,"isArchived":false,
        "forkedFromSessionId":LOCAL_ID,"spawnedFrom":{"sessionId":LOCAL_ID,"sideSession":true,"userFork":true}}).to_string()).unwrap();
    fs::write(app.join("claude_desktop_config.json"), json!({"preferences":{"epitaxyPrefs":{
        "dframe-group-scopes":{"acct/org":{"groups":[{"id":"cg-1","name":"项目"}],"assignments":{format!("code:{LOCAL_ID}"):"cg-1"}}},
        "dframe-code-sections":{"acct/org":{"sections":[
            {"id":"sessions","kind":"sessions","name":"","order":3},{"id":"pinned","kind":"pinned","name":"","order":0},
            {"id":"routines","kind":"routines","name":"","order":1},{"id":"cg-1","kind":"manual","name":"项目","order":2,"collapsed":true},
            {"id":"cg-2","kind":"manual","name":"隐藏","order":4,"shown":false}]}},
        "starred-local-code-sessions":[LOCAL_ID]}}}).to_string()).unwrap();
    let list = f.claude.dispatch("claude/thread/list", &json!({})).unwrap();
    let ids: Vec<&str> = list["sections"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["pinned", "cg-1", "sessions"]);
    assert_eq!(list["sections"][1]["collapsed"], true);
    let fork = list["data"].as_array().unwrap().iter().find(|t| t["id"] == fork_id).unwrap();
    assert_eq!(fork["parentId"], LOCAL_ID);
    assert_eq!(fork["lineage"], "fork");
    assert_eq!(fork["groupId"], "cg-1");
    assert_eq!(fork["updatedAtMs"], 1_793_588_400_000u64);
    let parent = list["data"].as_array().unwrap().iter().find(|t| t["id"] == LOCAL_ID).unwrap();
    assert_eq!(parent["pinned"], true);
    assert_eq!(parent["parentId"], Value::Null);
}

#[test]
fn reports_unread_waiting_and_searches_sent_messages() {
    let f = fixture();
    let app = f.root.path().join("Claude-3p");
    let record = app.join(format!("claude-code-sessions/acct/org/{LOCAL_ID}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    raw["lastTurnEdgeAt"] = json!(2_000u64);
    raw["lastFocusedAt"] = json!(1_000u64);
    fs::write(&record, raw.to_string()).unwrap();
    fs::create_dir_all(app.join("claude-code-sessions/acct/org/waiting-input")).unwrap();
    fs::write(app.join(format!("claude-code-sessions/acct/org/waiting-input/{LOCAL_ID}.json")), "{}").unwrap();
    let list = f.claude.dispatch("claude/thread/list", &json!({})).unwrap();
    let desktop = &list["data"][0]["desktop"];
    assert_eq!((desktop["turnEndedAt"].as_u64(), desktop["focusedAt"].as_u64(), desktop["waitingOnDesktop"].as_bool()), (Some(2_000), Some(1_000), Some(true)));
    let found = f.claude.dispatch("claude/thread/search", &json!({"query":"HELLO"})).unwrap();
    assert_eq!(found["data"][0]["id"], LOCAL_ID);
    assert_eq!(found["data"][0]["searchMatch"]["snippet"], "hello");
    assert_eq!(f.claude.dispatch("claude/thread/search", &json!({"query":"hi"})).unwrap()["total"], 0);
}

#[test]
fn delivers_to_the_process_the_desktop_runs() {
    use std::{io::{BufRead, BufReader, Write}, os::unix::net::UnixListener};
    let f = fixture();
    let sessions = f.root.path().join("claude-home/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let socket = std::path::PathBuf::from(format!("/tmp/clawkit-inbox-test-{}.sock", uuid::Uuid::new_v4().simple()));
    let listener = UnixListener::bind(&socket).unwrap();
    let pid = std::process::id();
    fs::write(sessions.join(format!("{pid}.json")), json!({"pid":pid,"sessionId":CLI_ID,"status":"idle","messagingSocketPath":socket}).to_string()).unwrap();
    fs::write(sessions.join(format!("{pid}.{}.key", "a".repeat(64))), json!({"peerToken":"0123456789abcdef0123456789abcdef"}).to_string()).unwrap();
    let list = f.claude.dispatch("claude/thread/list", &json!({})).unwrap();
    assert_eq!((list["data"][0]["desktop"]["status"].as_str(), list["data"][0]["desktop"]["canSend"].as_bool()), (Some("desktop-live"), Some(true)));
    let transcript = f.root.path().join("claude-home/projects").join(index::encode_project(&f.cwd)).join(format!("{CLI_ID}.jsonl"));
    let inbox = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let lines: Vec<Value> = BufReader::new(stream).lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect();
        let id = lines[1]["uuid"].as_str().unwrap().to_string();
        let mut file = fs::OpenOptions::new().append(true).open(&transcript).unwrap();
        writeln!(file, "\n{}", json!({"type":"user","uuid":id,"parentUuid":"a1","isMeta":true,"origin":{"kind":"peer","from":inbox::FROM},
            "timestamp":"2026-10-30T08:38:00.000Z","message":{"role":"user","content":format!("Another Claude session sent a message:\n{}\n\nThis came from another Claude session — not typed by your user.", lines[1]["message"]["content"].as_str().unwrap())}})).unwrap();
        lines
    });
    let request = uuid::Uuid::new_v4().to_string();
    let receipt = f.claude.dispatch("claude/turn/start", &json!({"threadId":LOCAL_ID,"requestId":request,"text":"从手机发到电脑"})).unwrap();
    let lines = inbox.join().unwrap();
    assert_eq!(lines[0], json!({"type":"auth","token":"0123456789abcdef0123456789abcdef"}));
    assert_eq!((lines[1]["from"].as_str(), lines[1]["uuid"].as_str()), (Some(inbox::FROM), Some(request.as_str())));
    assert_eq!((receipt["deliveredTo"].as_str(), receipt["recorded"].as_bool()), (Some("desktop"), Some(true)));
    assert_eq!(f.claude.dispatch("claude/turn/start", &json!({"threadId":LOCAL_ID,"requestId":request,"text":"从手机发到电脑"})).unwrap(), receipt);
    let read = f.claude.dispatch("claude/thread/read", &json!({"threadId":LOCAL_ID})).unwrap();
    let last = read["thread"]["turns"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["items"][0]["content"][0]["text"], "从手机发到电脑");
    let _ = fs::remove_file(&socket);
}
