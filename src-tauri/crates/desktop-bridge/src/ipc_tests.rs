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
