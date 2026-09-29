use crate::creation::{project, Creations};
use rusqlite::{params, Connection};
use serde_json::json;
#[test]
fn durable_creation_resolves_original_project_and_blocks_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let pid = "dcd73312-7e00-7151-bf92-d86ef155544a";
    let cwd = root.path().to_str().unwrap();
    let db = Connection::open(root.path().join("state_5.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE projects(id TEXT,name TEXT,position INTEGER);
        CREATE TABLE project_roots(project_id TEXT,path TEXT,position INTEGER);
        CREATE TABLE threads(id TEXT,first_user_message TEXT,project_id TEXT,created_at_ms INTEGER,cwd TEXT,source TEXT);").unwrap();
    db.execute("INSERT INTO projects VALUES(?1,'Test project',0)", [pid])
        .unwrap();
    db.execute(
        "INSERT INTO project_roots VALUES(?1,?2,0)",
        params![pid, cwd],
    )
    .unwrap();
    let state = json!({"local-projects":{"legacy":{"name":"Test project","rootPaths":[cwd]}},
        "app-server-project-id-by-legacy-project-id-by-host":{format!("local:{cwd}"):{"legacy":pid}},
        "thread-project-assignments":{"original":{"projectKind":"local","projectId":"legacy"}}});
    std::fs::write(
        root.path().join(".codex-global-state.json"),
        state.to_string(),
    )
    .unwrap();
    let p = project(root.path(), pid).unwrap();
    assert_eq!(p.legacy_id, "legacy");
    let ledger = Creations::open(root.path()).unwrap();
    assert!(ledger.not_sent("request"));
    ledger.begin("request", &p, "synthetic").unwrap();
    assert!(!ledger.not_sent("request"));
    assert!(ledger.begin("different", &p, "different").is_err());
    assert!(ledger.same("request", pid, "synthetic").unwrap());
    assert!(ledger.same("request", pid, "changed").is_err());
    assert_eq!(
        ledger.read(root.path(), "request", pid).unwrap()["status"],
        "unknown"
    );
    drop(ledger);
    db.execute(
        "INSERT INTO threads VALUES('original','synthetic',NULL,?1,?2,'vscode')",
        params![crate::now_ms(), cwd],
    )
    .unwrap();
    let ledger = Creations::open(root.path()).unwrap();
    let result = ledger.read(root.path(), "request", pid).unwrap();
    assert_eq!(result["status"], "accepted");
    assert_eq!(result["threadId"], "original");
    assert_eq!(ledger.read(root.path(), "request", pid).unwrap(), result);
    ledger.begin("next", &p, "next").unwrap();
    ledger.reject("next").unwrap();
    assert!(ledger.not_sent("next"));
    let native_before = std::fs::read(root.path().join(".codex-global-state.json")).unwrap();
    assert_eq!(native_before, state.to_string().as_bytes());
}
