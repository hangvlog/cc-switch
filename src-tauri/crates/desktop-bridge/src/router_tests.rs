use super::router::*;
use serde_json::json;
use std::{os::unix::net::UnixStream, sync::{Arc,Mutex}};
pub(crate) fn test_routing(
    client: UnixStream,
    owner: UnixStream,
    peer: &mut UnixStream,
    target: &mut UnixStream,
) {
    let mut state = State::default();
    state.clients.insert(
        "phone".into(),
        Client {
            writer: Arc::new(Mutex::new(client)),
        },
    );
    state.clients.insert(
        "desktop".into(),
        Client {
            writer: Arc::new(Mutex::new(owner)),
        },
    );
    deliver(route(
        &mut state,
        "phone",
        json!({"type":"request","requestId":"original","method":"thread-owner-discovery","version":1,"params":{"conversationId":"test"}}),
    ));
    let discovery = read(target).unwrap();
    assert_eq!(discovery["type"], "client-discovery-request");
    let route_id = discovery["requestId"].clone();
    deliver(route(
        &mut state,
        "desktop",
        json!({"type":"client-discovery-response","requestId":route_id,"response":{"canHandle":true}}),
    ));
    let request = read(target).unwrap();
    assert_eq!(request["requestId"], route_id);
    assert_eq!(request["sourceClientId"], "phone");
    assert!(route(
        &mut state,
        "phone",
        json!({"type":"response","requestId":route_id,"resultType":"success"})
    )
    .is_empty());
    deliver(route(
        &mut state,
        "desktop",
        json!({"type":"response","requestId":route_id,"resultType":"success","result":{"ok":true}}),
    ));
    let reply = read(peer).unwrap();
    assert_eq!(reply["requestId"], "original");
    assert_eq!(reply["handledByClientId"], "desktop");
    deliver(route(
        &mut state,
        "desktop",
        json!({"type":"broadcast","method":"thread-read-state-changed","version":3,"targetClientIds":["phone"],"params":{"hasUnreadTurn":false}}),
    ));
    let changed = read(peer).unwrap();
    assert_eq!(changed["sourceClientId"], "desktop");
    assert_eq!(changed["version"], 3);
    assert!(state.pending.is_empty());
}
