//! Ordinary-launch owner updates. Full state remains local; only the existing
//! bounded visible projection reaches the relay. Latest-state delivery coalesces
//! bursts, and every reconnect starts from a new verified snapshot.
use crate::{attachments::Attachments, ipc::IpcClient};
use serde_json::{json, Value};
use std::{collections::HashMap, io::Read, path::{Path, PathBuf}, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

struct Entry {
    watchers: HashMap<String, (Instant, bool)>,
    generation: String,
    stop: Arc<AtomicBool>,
    latest: Option<Value>,
    pending: bool,
    sequence: u64,
    fingerprint: String,
}
struct Inner { entries: Mutex<HashMap<String, Entry>>, enabled: Arc<AtomicBool> }
#[derive(Clone)]
pub struct OwnerStreams(Arc<Inner>);
impl OwnerStreams {
    pub fn new(enabled: Arc<AtomicBool>) -> Self {
        Self(Arc::new(Inner { entries: Mutex::new(HashMap::new()), enabled }))
    }
    pub fn watch(&self, home: &Path, storage: &Path, thread: &str, watch: &str, latest_only: bool) -> Result<Value,String> {
        uuid::Uuid::parse_str(watch).map_err(|_|"Invalid watch ID")?;
        let mut entries=self.0.entries.lock().map_err(|_|"Owner stream unavailable")?;
        entries.retain(|_,e| {e.watchers.retain(|_,t|t.0.elapsed()<Duration::from_secs(90));let keep=!e.watchers.is_empty();if !keep {e.stop.store(true,Ordering::SeqCst);}keep});
        if let Some(entry)=entries.get_mut(thread) {
            if entry.watchers.len()>=16 && !entry.watchers.contains_key(watch) {return Err("Too many watch leases".into());}
            entry.watchers.insert(watch.into(),(Instant::now(), latest_only));
            let mut latest=entry.latest.clone();
            if latest_only { if let Some(thread)=latest.as_mut() {crate::latest_message::compact(thread);} }
            return Ok(json!({"generation":entry.generation,"thread":latest,"sequence":entry.sequence}));
        }
        if entries.len()>=2 {return Err("同时同步的会话过多，请稍后重试".into());}
        let generation=uuid::Uuid::new_v4().to_string();
        let stop=Arc::new(AtomicBool::new(false));
        entries.insert(thread.into(),Entry {watchers:HashMap::from([(watch.to_owned(),(Instant::now(), latest_only))]),generation:generation.clone(),stop:stop.clone(),latest:None,pending:false,sequence:0,fingerprint:String::new()});
        let (this,home,storage,thread)=(self.clone(),home.to_owned(),storage.to_owned(),thread.to_owned());
        std::thread::spawn(move || this.run(home,storage,thread,stop));
        Ok(json!({"generation":generation,"thread":null,"sequence":0}))
    }
    pub fn unwatch(&self, thread: &str, watch: &str) {
        if let Ok(mut entries)=self.0.entries.lock() {if let Some(e)=entries.get_mut(thread){e.watchers.remove(watch);if e.watchers.is_empty(){e.stop.store(true,Ordering::SeqCst);entries.remove(thread);}}}
    }
    pub fn drain(&self) -> Vec<Value> {
        if !self.0.enabled.load(Ordering::SeqCst) {return vec![];}
        self.0.entries.lock().map(|mut entries|entries.iter_mut().filter_map(|(id,e)|{
            if !e.pending {return None;}e.pending=false;
            let mut latest=e.latest.clone();
            if e.watchers.values().all(|v|v.1) { if let Some(thread)=latest.as_mut() {crate::latest_message::compact(thread);} }
            Some(json!({"method":"desktop/owner/update","params":{"threadId":id,"generation":e.generation,"thread":latest,"sequence":e.sequence}}))
        }).collect()).unwrap_or_default()
    }
    fn running(&self,thread:&str,stop:&AtomicBool)->bool {
        !stop.load(Ordering::SeqCst) && self.0.enabled.load(Ordering::SeqCst)
            && self.0.entries.lock().is_ok_and(|e|e.get(thread).is_some_and(|e|e.watchers.values().any(|t|t.0.elapsed()<Duration::from_secs(90))))
    }
    fn publish(&self,thread:&str,stop:&AtomicBool,visible:Value) {
        if stop.load(Ordering::SeqCst){return;}
        if let Ok(mut entries)=self.0.entries.lock() {if let Some(e)=entries.get_mut(thread) {
            if !stop.load(Ordering::SeqCst) && !e.stop.load(Ordering::SeqCst) {
                let projected=crate::conditional_read::response(json!({"thread":visible}),None);
                let fingerprint=projected["version"].as_str().unwrap_or_default();
                if e.fingerprint!=fingerprint {e.fingerprint=fingerprint.into();e.sequence+=1;e.latest=Some(projected["thread"].clone());e.pending=true;}
            }
        }}
    }
    fn run(&self,home:PathBuf,storage:PathBuf,thread:String,stop:Arc<AtomicBool>) {
        let Ok(store)=Attachments::open(&storage) else{return};
        while self.running(&thread,&stop) {
            if self.session(&home,&thread,&store,&stop).is_err() && self.running(&thread,&stop) {
                // Do not leave stale send/stop state looking live after a gap.
                self.publish(&thread,&stop,json!({"id":thread,"turns":[],"desktop":{
                    "source":"history","status":"unknown","canSend":false,"canSteer":false,
                    "reason":"正在恢复实时同步"}}));
            }
            for _ in 0..10 {if !self.running(&thread,&stop){break;}std::thread::sleep(Duration::from_millis(100));}
        }
    }
    fn session(&self,home:&Path,thread:&str,store:&Attachments,stop:&AtomicBool)->Result<(),String> {
        let mut client=IpcClient::connect(home)?;
        let discovery=client.request("thread-owner-discovery",json!({"hostId":"local","conversationId":thread}),1,None)?;
        let owner=discovery["handledByClientId"].as_str().ok_or("Owner missing")?.to_owned();
        client.follow(thread,&owner,true)?;
        #[cfg(any(unix,windows))]
        client.stream.set_read_timeout(Some(Duration::from_millis(250))).map_err(|e|e.to_string())?;
        let mut reader=Frames::default();let mut state=State::default();
        let mut last=Instant::now()-Duration::from_secs(1);let mut dirty=false;
        while self.running(thread,stop) {
            match reader.next(&mut client) {
                Ok(Some(event)) => {
                    client.decline_discovery(&event)?;
                    if event["method"]=="client-status-changed" && event["params"]["clientId"]==owner && event["params"]["status"]=="disconnected" {return Err("Owner disconnected".into());}
                    if event["method"]=="thread-stream-following-status-requested" && event["params"]["conversationId"]==thread {client.follow(thread,&owner,true)?;}
                    if event["method"]=="thread-stream-state-changed" && event["sourceClientId"]==owner && event["params"]["conversationId"]==thread {
                        if event["version"]!=11 {return Err("Owner stream protocol changed".into());}
                        state.apply(thread,&event["params"]["change"])?;dirty=true;
                    }
                }
                Ok(None)=>{},Err(e)=>return Err(e),
            }
            if dirty && last.elapsed()>=Duration::from_millis(150) {
                let visible=crate::history::visible_thread(&state.value,&owner,state.revision,store);
                self.publish(thread,stop,visible);dirty=false;last=Instant::now();
            }
        }
        let _=client.follow(thread,&owner,false);Ok(())
    }
}

#[derive(Default)] struct State {value:Value,revision:u64,ready:bool}
impl State {
    fn apply(&mut self,thread:&str,change:&Value)->Result<(),String> {
        let revision=change["revision"].as_u64().ok_or("Owner revision missing")?;
        match change["type"].as_str() {
            Some("snapshot")=>{self.value=change["conversationState"].clone();self.ready=true;},
            Some("patches") if self.ready && change["baseRevision"].as_u64()==Some(self.revision) && revision>self.revision => {
                if let Err(e)=crate::owner_patches::apply(&mut self.value,&change["patches"]) {self.ready=false;return Err(e);}
            },
            _=>{self.ready=false;return Err("Owner stream gap; fresh snapshot required".into());},
        }
        if self.value["id"]!=thread {self.ready=false;return Err("Owner snapshot identity mismatch".into());}
        self.revision=revision;Ok(())
    }
}

// Retain partial framing across read timeouts; never interpret a body fragment
// as the next length header. A stalled partial frame forces reconnection.
#[derive(Default)] struct Frames {bytes:Vec<u8>,expected:Option<usize>,started:Option<Instant>}
impl Frames {
    fn next(&mut self,client:&mut IpcClient)->Result<Option<Value>,String> {
        if self.started.is_some_and(|t|t.elapsed()>Duration::from_secs(12)){return Err("Owner frame stalled".into());}
        let goal=self.expected.unwrap_or(4);
        let mut part=vec![0u8;goal.saturating_sub(self.bytes.len()).min(64*1024)];
        match client.stream.read(&mut part) {
            Ok(0)=>return Err("Owner disconnected".into()),
            Ok(n)=>{self.started.get_or_insert_with(Instant::now);self.bytes.extend_from_slice(&part[..n]);},
            Err(e) if matches!(e.kind(),std::io::ErrorKind::TimedOut|std::io::ErrorKind::WouldBlock)=>return Ok(None),
            Err(e)=>return Err(e.to_string()),
        }
        if self.bytes.len()!=goal{return Ok(None);}
        if self.expected.is_none() {
            let size=u32::from_le_bytes(self.bytes[..4].try_into().unwrap()) as usize;
            if size==0||size>256*1024*1024{return Err("Invalid owner frame length".into());}
            self.bytes.clear();self.expected=Some(size);return Ok(None);
        }
        let result=serde_json::from_slice(&self.bytes).map_err(|_|"Invalid owner frame JSON".to_owned());
        self.bytes.clear();self.expected=None;self.started=None;result.map(Some)
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[cfg(unix)]
    #[test] fn partial_frame_survives_timeout_and_visible_patch_is_delivered() {
        use std::{io::{Read,Write},os::unix::net::UnixListener};
        let root=tempfile::tempdir().unwrap();std::fs::create_dir(root.path().join("ipc")).unwrap();
        let listener=UnixListener::bind(root.path().join("ipc/ipc.sock")).unwrap();
        let server=std::thread::spawn(move||{
            let (mut s,_)=listener.accept().unwrap();
            let mut size=[0;4];s.read_exact(&mut size).unwrap();let mut b=vec![0;u32::from_le_bytes(size) as usize];s.read_exact(&mut b).unwrap();let init:Value=serde_json::from_slice(&b).unwrap();
            let b=json!({"type":"response","requestId":init["requestId"],"resultType":"success","result":{"clientId":"f"}}).to_string().into_bytes();s.write_all(&(b.len() as u32).to_le_bytes()).unwrap();s.write_all(&b).unwrap();
            let b=json!({"method":"test","payload":"fragmented"}).to_string().into_bytes();
            let mut frame=(b.len() as u32).to_le_bytes().to_vec();frame.extend(b);
            s.write_all(&frame[..2]).unwrap();std::thread::sleep(Duration::from_millis(100));s.write_all(&frame[2..8]).unwrap();std::thread::sleep(Duration::from_millis(100));s.write_all(&frame[8..]).unwrap();
        });
        let mut c=IpcClient::connect(root.path()).unwrap();c.stream.set_read_timeout(Some(Duration::from_millis(30))).unwrap();let mut reader=Frames::default();let mut got=None;
        for _ in 0..30 {if let Some(v)=reader.next(&mut c).unwrap(){got=Some(v);break;}}
        assert_eq!(got.unwrap()["payload"],"fragmented");server.join().unwrap();
        let enabled=Arc::new(AtomicBool::new(true));let streams=OwnerStreams::new(enabled);
        let stop=Arc::new(AtomicBool::new(false));streams.0.entries.lock().unwrap().insert("t".into(),Entry {watchers:HashMap::new(),generation:"g".into(),stop:stop.clone(),latest:None,pending:false,sequence:0,fingerprint:String::new()});
        streams.publish("t",&stop,json!({"id":"t","turns":[],"desktop":{"revision":1,"observedAt":1}}));assert_eq!(streams.drain().len(),1);
        streams.publish("t",&stop,json!({"id":"t","turns":[],"desktop":{"revision":2,"observedAt":2}}));assert!(streams.drain().is_empty(),"Internal-only changes do not repeat payloads");
        streams.publish("t",&stop,json!({"id":"t","turns":[{"id":"u"}],"desktop":{"revision":3}}));assert_eq!(streams.drain()[0]["params"]["sequence"],2);
    }
    #[test] fn revision_gap_and_identity_change_require_fresh_snapshot() {
        let mut s=State::default();s.apply("t",&json!({"type":"snapshot","revision":4,"conversationState":{"id":"t","text":"a"}})).unwrap();
        s.apply("t",&json!({"type":"patches","baseRevision":4,"revision":5,"patches":[{"op":"replace","path":["text"],"value":"ab"}]})).unwrap();assert_eq!(s.value["text"],"ab");
        assert!(s.apply("t",&json!({"type":"patches","baseRevision":6,"revision":7,"patches":[]})).is_err());assert!(!s.ready);
        assert!(s.apply("t",&json!({"type":"snapshot","revision":9,"conversationState":{"id":"other"}})).is_err());
    }
}
