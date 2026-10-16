use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
#[test]
fn native_creation_is_account_scoped_and_reconciles_only_exact_client_id() {
    // Keep Unix socket paths short on macOS.
    let dir = tempfile::Builder::new()
        .prefix("ck-native-")
        .tempdir_in("/tmp")
        .unwrap();
    let home = dir.path();
    std::fs::create_dir(home.join("clawkit-runtime")).unwrap();
    std::fs::set_permissions(
        home.join("clawkit-runtime"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let socket = socket_path(home);
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let create_count = Arc::new(AtomicUsize::new(0));
    let count = create_count.clone();
    let project = uuid::Uuid::new_v4().to_string();
    let thread = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let (p, t, r, cwd) = (
        project.clone(),
        thread.clone(),
        id.clone(),
        home.to_string_lossy().to_string(),
    );
    let server = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "clawkit/status" => json!({"result":{"ready":true,"protocolVersion":1}}),
                "project/read" => json!({"result":{"project":{"id":p,"roots":[{"path":cwd}]}}}),
                "thread/start" => {
                    count.fetch_add(1, Ordering::SeqCst);
                    json!({"result":{"thread":{"id":t,"projectId":p,"cwd":cwd}}})
                }
                // Simulate lost first-turn response after the engine has accepted it.
                "turn/start" => json!({"error":{"code":-32002}}),
                "thread/read" => {
                    json!({"result":{"thread":{"id":t,"projectId":p,"turns":[{"items":[{"type":"userMessage","clientId":r}]}]}}})
                }
                "stop" => break,
                _ => panic!("unexpected request"),
            };
            writeln!(stream, "{result}").unwrap();
        }
    });
    let storage = home.join("account-a");
    let ledger = creation::NativeCreations::open(&storage).unwrap();
    let enabled = AtomicBool::new(true);
    let accepted = ledger
        .create(
            home,
            &id,
            &project,
            "synthetic",
            json!([{"type":"text","text":"synthetic","text_elements":[]}]),
            &enabled,
        )
        .unwrap();
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["threadId"], thread);
    assert_eq!(
        ledger
            .create(
                home,
                &id,
                &project,
                "synthetic",
                json!([{"type":"text","text":"synthetic","text_elements":[]}]),
                &enabled
            )
            .unwrap(),
        accepted
    );
    assert!(ledger
        .create(home, &id, &project, "different", json!([]), &enabled)
        .is_err());
    assert_eq!(create_count.load(Ordering::SeqCst), 1);
    drop(ledger);
    assert_eq!(
        creation::NativeCreations::open(&storage)
            .unwrap()
            .read(home, &id, &project)
            .unwrap(),
        accepted
    );
    assert!(!creation::NativeCreations::open(&home.join("account-b"))
        .unwrap()
        .exists(&id));
    let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
    writeln!(stream, "{{\"method\":\"stop\"}}").unwrap();
    server.join().unwrap();
}
