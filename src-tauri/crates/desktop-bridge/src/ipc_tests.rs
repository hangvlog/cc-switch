use crate::ipc::IpcClient;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};

fn read(stream: &mut UnixStream) -> Value {
    let mut size = [0; 4];
    stream.read_exact(&mut size).unwrap();
    let mut body = vec![0; u32::from_le_bytes(size) as usize];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}
fn write(stream: &mut UnixStream, value: Value) {
    let body = serde_json::to_vec(&value).unwrap();
    stream
        .write_all(&(body.len() as u32).to_le_bytes())
        .unwrap();
    // Exercise a fragmented response, as real sockets do not preserve frame boundaries.
    for chunk in body.chunks(17) {
        stream.write_all(chunk).unwrap();
    }
}
fn reply(stream: &mut UnixStream, request: &Value, result: Value) {
    write(
        stream,
        json!({"type":"response","requestId":request["requestId"],"resultType":"success","handledByClientId":"owner","result":result}),
    );
}

#[test]
fn follows_original_owner_and_inherits_settings_without_claiming_ownership() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("ipc")).unwrap();
    let listener = UnixListener::bind(root.path().join("ipc/ipc.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let init = read(&mut stream);
        assert_eq!(init["method"], "initialize");
        reply(&mut stream, &init, json!({"clientId":"follower"}));
        let discovery = read(&mut stream);
        assert_eq!(discovery["method"], "thread-owner-discovery");
        reply(
            &mut stream,
            &discovery,
            json!({"supportsUntrustedAppInput":true}),
        );
        let follow = read(&mut stream);
        assert_eq!(follow["params"]["following"], true);
        assert_eq!(follow["targetClientIds"], json!(["owner"]));
        write(
            &mut stream,
            json!({"type":"broadcast","method":"thread-stream-state-changed","sourceClientId":"owner","version":11,
            "params":{"conversationId":"thread","change":{"type":"snapshot","revision":4,"conversationState":{"id":"thread"}}}}),
        );
        let unfollow = read(&mut stream);
        assert_eq!(unfollow["params"]["following"], false);
        let send = read(&mut stream);
        assert_eq!(send["method"], "thread-follower-start-turn");
        assert_eq!(send["targetClientId"], "owner");
        assert_eq!(
            send["params"]["turnStart"]["context"],
            json!({"inheritThreadSettings":true})
        );
        assert_eq!(
            send["params"]["turnStart"]["request"]["clientUserMessageId"],
            "request"
        );
        assert!(send["params"]["turnStart"]["request"]
            .get("model")
            .is_none());
        reply(&mut stream, &send, json!({"result":{"turn":{"id":"turn"}}}));
    });
    let mut client = IpcClient::connect(root.path()).unwrap();
    let snapshot = client.snapshot("thread", None).unwrap();
    assert_eq!(snapshot.revision, 4);
    assert!(client
        .start_turn("thread", &snapshot.owner, "request", "hello")
        .is_ok());
    server.join().unwrap();
}

#[test]
fn opens_dormant_thread_in_original_desktop_after_router_discovery_finishes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("ipc")).unwrap();
    let listener = UnixListener::bind(root.path().join("ipc/ipc.sock")).unwrap();
    let (opened, wait_open) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let init = read(&mut stream);
        reply(&mut stream, &init, json!({"clientId":"follower"}));
        let discovery = read(&mut stream);
        assert_eq!(discovery["method"], "thread-owner-discovery");
        // The real router has a 10s discovery timeout, longer than the old 5s reader.
        std::thread::sleep(std::time::Duration::from_millis(10_050));
        write(
            &mut stream,
            json!({"type":"response","requestId":discovery["requestId"],
            "resultType":"error","error":"no-client-found"}),
        );
        wait_open.recv().unwrap();
        drop(stream);
        // Opening is asynchronous: discovery can start before the desktop resumes.
        // That stale discovery will not acquire the owner which appears afterward.
        let (mut loading, _) = listener.accept().unwrap();
        let init = read(&mut loading);
        reply(&mut loading, &init, json!({"clientId":"loading-follower"}));
        assert_eq!(read(&mut loading)["method"], "thread-owner-discovery");
        std::thread::sleep(std::time::Duration::from_millis(2100));
        drop(loading);
        let (mut stream, _) = listener.accept().unwrap();
        let init = read(&mut stream);
        reply(&mut stream, &init, json!({"clientId":"fresh-follower"}));
        let discovery = read(&mut stream);
        reply(&mut stream, &discovery, json!({}));
        let follow = read(&mut stream);
        assert_eq!(follow["params"]["following"], true);
        assert_eq!(follow["targetClientIds"], json!(["owner"]));
        write(
            &mut stream,
            json!({"type":"broadcast","method":"thread-stream-state-changed",
            "sourceClientId":"owner","version":11,"params":{"conversationId":"dormant",
            "change":{"type":"snapshot","revision":1,"conversationState":{"id":"dormant",
            "resumeState":"resumed","requests":[],"threadRuntimeStatus":{"type":"idle"}}}}}),
        );
        assert_eq!(read(&mut stream)["params"]["following"], false);
    });
    let snapshot = IpcClient::open_snapshot_with(root.path(), "dormant", || {
        opened.send(()).unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(snapshot.owner, "owner");
    assert_eq!(snapshot.state["resumeState"], "resumed");
    server.join().unwrap();
}

#[test]
fn incompatible_or_broken_ipc_does_not_trigger_navigation() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("ipc")).unwrap();
    let listener = UnixListener::bind(root.path().join("ipc/ipc.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let init = read(&mut stream);
        reply(&mut stream, &init, json!({"clientId":"follower"}));
        let discovery = read(&mut stream);
        write(
            &mut stream,
            json!({"type":"response","requestId":discovery["requestId"],
            "resultType":"error","error":"unsupported-version"}),
        );
    });
    let result = IpcClient::open_snapshot_with(root.path(), "thread", || {
        panic!("A protocol failure must not navigate or start another executor")
    });
    assert!(result.is_err());
    server.join().unwrap();
}
