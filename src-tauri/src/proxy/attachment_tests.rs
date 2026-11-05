use super::*;
use axum::{
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
#[serial_test::serial]
async fn oversized_tool_request_is_bounded_cached_and_atomic_on_failure() {
    let home = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
    std::env::set_var("CC_SWITCH_TEST_HOME", home.path());
    crate::settings::reload_settings().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "http://{}/v1/clawkit/attachments",
        listener.local_addr().unwrap()
    );
    let advertised = base.clone();
    let uploads = count.clone();
    let app=Router::new().route("/v1/clawkit/attachments/capabilities",get(||async{Json(json!({"enabled":true,"protocol":"responses","models":["gpt-6.1-sol"],"limit_bytes":40<<20,"target_bytes":38<<20,"max_attachment_bytes":20<<20}))}))
 .route("/v1/clawkit/attachments/prepare",post(move ||{let base=advertised.clone();let uploads=uploads.clone();async move {uploads.fetch_add(1,Ordering::SeqCst);Json(json!({"id":"test","url":format!("{base}/test/content?e=123&s=fixture"),"expires":now()+1800}))}}));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        "Bearer synthetic-attachment-test".parse().unwrap(),
    );
    let mut images = Vec::new();
    for i in 0..44 {
        images.push(json!({"type":"input_image","image_url":format!("data:image/png;base64,{}",STANDARD.encode(vec![i;716800])),"detail":"high"}));
    }
    let body = json!({"model":"gpt-6.1-sol","stream":true,"store":false,"input":[{"type":"custom_tool_call_output","id":"ctco_fixture","call_id":"call_fixture","output":images}],"tools":[{"type":"namespace","name":"untouched"}]});
    let serialized = serde_json::to_vec(&body).unwrap();
    assert!(serialized.len() > 40 << 20);
    let mut optimized = body.clone();
    let result = prepare_with_endpoint(&mut optimized, serialized.clone(), &base, &headers)
        .await
        .unwrap();
    assert!(result.len() <= 38 << 20);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert_eq!(optimized["tools"], body["tools"]);
    assert_eq!(optimized["input"][0]["id"], body["input"][0]["id"]);
    assert_eq!(
        optimized["input"][0]["output"].as_array().unwrap().len(),
        44
    );
    let mut repeat = body.clone();
    let cached = prepare_with_endpoint(&mut repeat, serialized, &base, &headers)
        .await
        .unwrap();
    assert_eq!(result, cached);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    let mut text = json!({"model":"gpt-6.1-sol","input":"x".repeat((40<<20)+1)});
    let original = text.clone();
    let bytes = serde_json::to_vec(&text).unwrap();
    assert!(matches!(
        prepare_with_endpoint(&mut text, bytes, &base, &headers).await,
        Err(ProxyError::RequestBodyTooLarge(_))
    ));
    assert_eq!(text, original);
    let failing_base = format!("{base}/failure");
    let mut failed = body.clone();
    let bytes = serde_json::to_vec(&failed).unwrap();
    assert!(matches!(
        prepare_with_endpoint(&mut failed, bytes, &failing_base, &headers).await,
        Err(ProxyError::AttachmentUpload(_))
    ));
    assert_eq!(failed, body);
    server.abort();
    match previous {
        Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
        None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
    };
    crate::settings::reload_settings().unwrap();
}

/// Opt-in acceptance through the public gateway; synthetic images only.
#[tokio::test]
#[serial_test::serial]
async fn live_gateway_preserves_44_images_and_reuses_receipts() {
    let Ok(directory) = std::env::var("CLAWKIT_ATTACHMENT_LIVE_FIXTURES") else { return; };
    let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("CC_SWITCH_TEST_HOME", home.path());
    crate::settings::reload_settings().unwrap();
    let bearer = std::fs::read_to_string(std::path::Path::new(&directory).join("api-token.private")).unwrap();
    let mut headers = http::HeaderMap::new();
    headers.insert(http::header::AUTHORIZATION, format!("Bearer {}",bearer.trim()).parse().unwrap());
    let mut images = Vec::new();
    for index in 0..44 {
        let data = std::fs::read(std::path::Path::new(&directory).join(format!("image-{index}.png"))).unwrap();
        images.push(json!({"type":"input_image","image_url":format!("data:image/png;base64,{}",STANDARD.encode(data)),"detail":"high"}));
    }
    let original = json!({"model":"gpt-6.1-sol","stream":true,"store":false,"input":[{"role":"user","content":[{"type":"input_text","text":"All images are synthetic fixtures. Read the code and rectangle color. Output only the code and color."}]},{"type":"custom_tool_call","id":"ctc_attachment_fixture","call_id":"call_attachment_fixture","name":"inspect_asset","input":"show images"},{"type":"custom_tool_call_output","id":"ctco_attachment_fixture","call_id":"call_attachment_fixture","output":images}]});
    for round in 1..=2 {
        let mut body = original.clone();
        let bytes = serde_json::to_vec(&body).unwrap();
        let input_bytes=bytes.len(); let start=Instant::now();
        let optimized=prepare(&mut body,bytes,"https://api.clawkit.chat/v1/responses",&headers).await.unwrap();
        let prepare_ms=start.elapsed().as_millis();
        assert!(optimized.len() <= 38<<20);
        assert_eq!(body["input"][2]["output"].as_array().unwrap().len(),44);
        let inference_client=reqwest::Client::builder().timeout(Duration::from_secs(240)).build().unwrap();
        let response=inference_client.post("https://api.clawkit.chat/v1/responses").headers(headers.clone()).header("Content-Type","application/json").body(optimized.clone()).send().await.unwrap();
        let status=response.status();let text=response.text().await.unwrap();
        let completed=text.lines().filter_map(|s|s.strip_prefix("data: ")).filter_map(|s|serde_json::from_str::<Value>(s).ok()).find(|v|v["type"]=="response.completed");
        let recognized=completed.as_ref().is_some_and(|v|v.to_string().contains("K7M3Q9"));
        println!("live round={round} input_bytes={input_bytes} output_bytes={} prepare_ms={prepare_ms} http_status={} completed={} recognized={recognized}",optimized.len(),status.as_u16(),completed.is_some());
        assert!(status.is_success() && recognized && completed.is_some(),"live fixture did not complete");
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    match previous {Some(v)=>std::env::set_var("CC_SWITCH_TEST_HOME",v),None=>std::env::remove_var("CC_SWITCH_TEST_HOME")};
    crate::settings::reload_settings().unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn live_http_proxy_offloads_44_images() {
    let Ok(directory) = std::env::var("CLAWKIT_ATTACHMENT_LIVE_FIXTURES") else { return; };
    let home = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
    std::env::set_var("CC_SWITCH_TEST_HOME", home.path());
    crate::settings::reload_settings().unwrap();
    let bearer = std::fs::read_to_string(std::path::Path::new(&directory).join("api-token.private")).unwrap();
    let db = Arc::new(crate::database::Database::memory().unwrap());
    let mut provider = crate::provider::Provider::with_id("attachment-live".into(), "Attachment acceptance".into(),
        json!({"base_url":"https://api.clawkit.chat/v1","auth":{"OPENAI_API_KEY":bearer.trim()}}),None);
    provider.meta=Some(crate::provider::ProviderMeta{api_format:Some("openai_responses".into()),..Default::default()});
    db.save_provider("codex",&provider).unwrap();
    db.set_current_provider("codex",&provider.id).unwrap();
    let proxy=crate::proxy::server::ProxyServer::new(crate::proxy::ProxyConfig{listen_port:0,enable_logging:false,max_retries:0,streaming_first_byte_timeout:120,streaming_idle_timeout:240,..Default::default()},db,None);
    let info=proxy.start().await.unwrap();
    let images:Vec<Value>=(0..44).map(|i| {let bytes=std::fs::read(std::path::Path::new(&directory).join(format!("image-{i}.png"))).unwrap();json!({"type":"input_image","image_url":format!("data:image/png;base64,{}",STANDARD.encode(bytes)),"detail":"high"})}).collect();
    let body=json!({"model":"gpt-6.1-sol","reasoning":{"effort":"low"},"stream":true,"store":false,"input":[{"role":"user","content":[{"type":"input_text","text":"Read the code and color in the synthetic images; answer in one short line."}]},{"type":"custom_tool_call","id":"ctc_live_http","call_id":"call_live_http","name":"inspect_asset","input":"show images"},{"type":"custom_tool_call_output","id":"ctco_live_http","call_id":"call_live_http","output":images}]});
    let input_bytes=serde_json::to_vec(&body).unwrap().len();assert!(input_bytes>40<<20);
    let started=Instant::now();
    let response=reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(300)).build().unwrap().post(format!("http://127.0.0.1:{}/v1/responses",info.port)).header("Authorization","Bearer synthetic-local-client").json(&body).send().await.unwrap();
    let status=response.status();let text=response.text().await.unwrap();
    let completed=text.lines().filter_map(|s|s.strip_prefix("data: ")).filter_map(|s|serde_json::from_str::<Value>(s).ok()).find(|v|v["type"]=="response.completed");
    let recognized=completed.as_ref().is_some_and(|v|v.to_string().contains("K7M3Q9"));
    println!("live_http input_bytes={input_bytes} http_status={} completed={} recognized={recognized} duration_ms={}",status.as_u16(),completed.is_some(),started.elapsed().as_millis());
    proxy.stop().await.unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    match previous{Some(v)=>std::env::set_var("CC_SWITCH_TEST_HOME",v),None=>std::env::remove_var("CC_SWITCH_TEST_HOME")};
    crate::settings::reload_settings().unwrap();
    assert!(status.is_success()&&completed.is_some()&&recognized,"local HTTP proxy fixture failed");
}
