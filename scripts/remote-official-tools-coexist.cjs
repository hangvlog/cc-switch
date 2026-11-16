// macOS integration check: unmodified official desktop, temporary profile, inherited CDP pipe.
// Uses the official app-server RPC for MCP discovery; never patches the host or bypasses authorization.
const fs = require('fs'), path = require('path'), net = require('net');
const {spawn, execFileSync} = require('child_process');
const assert = require('assert/strict');
const adapter = process.env.CLAWKIT_TEST_ADAPTER;
if (!adapter || process.platform !== 'darwin') throw Error('Set CLAWKIT_TEST_ADAPTER on macOS');
const home = fs.mkdtempSync('/tmp/ck-coexist-'); fs.chmodSync(home, 0o700);
const profile = path.join(home, 'profile'); fs.mkdirSync(profile, {mode:0o700});
// Reuse the installed, unmodified official plugin launcher in an isolated configuration.
// Its default bundled server is disabled until the app enables it for a chat.
const plugin='/Users/hang/.codex/plugins/cache/openai-bundled/codex-app-tools/26.1111.21308';
const mcp=JSON.parse(fs.readFileSync(path.join(plugin,'.mcp.json'))).mcpServers.codex_app;
fs.writeFileSync(path.join(home,'config.toml'), `approval_policy="never"\nsandbox_mode="read-only"\n[mcp_servers.codex_app]\ncommand=${JSON.stringify(mcp.command)}\ncwd=${JSON.stringify(mcp.cwd)}\nargs=${JSON.stringify(mcp.args)}\nenv_vars=${JSON.stringify(mcp.env_vars)}\nenabled=true\nstartup_timeout_sec=15\n[mcp_servers.codex_app.env]\nCODEX_APP_TOOLS_DEFERRED="1"\n`);
const app = '/Applications/ChatGPT.app/Contents/MacOS/ChatGPT';
const engine = '/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex';
const child = spawn(app, ['--remote-debugging-pipe', '--user-data-dir='+profile], {
  env:{...process.env,CODEX_HOME:home,CODEX_ELECTRON_USER_DATA_PATH:profile,
    CODEX_CLI_PATH:adapter,CLAWKIT_CODEX_ENGINE:engine},
  stdio:['ignore','ignore',fs.openSync(path.join(home,'desktop-stderr.log'),'a'),'pipe','pipe']
});
let seq=0, buffer=Buffer.alloc(0); const pending=new Map();
child.stdio[4].on('data', data=>{
  buffer=Buffer.concat([buffer,data]);
  let end;
  while ((end=buffer.indexOf(0))>=0) {
    const raw=buffer.subarray(0,end); buffer=buffer.subarray(end+1);
    if (!raw.length) continue;
    const result=JSON.parse(raw), p=pending.get(result.id);
    if (p) {pending.delete(result.id); clearTimeout(p.timer); result.error?p.reject(Error(JSON.stringify(result.error))):p.resolve(result.result);}
  }
});
function cdp(method,params={},sessionId) {
  return new Promise((resolve,reject)=>{
    const id=++seq, timer=setTimeout(()=>{pending.delete(id);reject(Error('CDP timeout '+method))},55000);
    pending.set(id,{resolve,reject,timer});
    child.stdio[3].write(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})})+'\0');
  });
}
const delay=ms=>new Promise(r=>setTimeout(r,ms));
async function evaluate(sessionId,expression) {
  const value=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true},sessionId);
  if(value.exceptionDetails) throw Error(JSON.stringify(value.exceptionDetails));
  return value.result.value;
}
function local(method,params={}) {
  return new Promise((resolve,reject)=>{
    const socket=net.connect(path.join(home,'clawkit-runtime/engine.sock'));
    let raw=''; socket.setTimeout(3000,()=>socket.destroy(Error('Remote endpoint timeout')));
    socket.on('error',reject); socket.on('data',data=>{raw+=data;if(raw.includes('\n')){socket.end();const value=JSON.parse(raw.trim());value.error?reject(Error(JSON.stringify(value.error))):resolve(value.result)}});
    socket.on('connect',()=>socket.write(JSON.stringify({method,params})+'\n'));
  });
}
let testThread;
(async()=>{
  let sessionId;
  for(let i=0;i<100;i++) {
    const targets=await cdp('Target.getTargets');
    const target=targets.targetInfos.find(t=>t.type==='page'&&t.url==='app://-/index.html');
    if(target) {sessionId=(await cdp('Target.attachToTarget',{targetId:target.targetId,flatten:true})).sessionId;break}
    await delay(100);
  }
  assert(sessionId,'official page');
  for(let i=0;i<100;i++) {if(await evaluate(sessionId,'!!globalThis.electronBridge?.sendMessageFromView'))break;await delay(100)}
  await evaluate(sessionId,`globalThis.__coexistRequest=(method,params)=>new Promise((resolve,reject)=>{
    const id='coexist-'+crypto.randomUUID();
    const listener=event=>{const value=event.data;if(value?.type==='mcp-response'&&value.message?.id===id){clearTimeout(timer);removeEventListener('message',listener);value.message.error?reject(Error(JSON.stringify(value.message.error))):resolve(value.message.result)}};
    const timer=setTimeout(()=>{removeEventListener('message',listener);reject(Error('App RPC timeout '+method))},45000);
    addEventListener('message',listener);electronBridge.sendMessageFromView({type:'mcp-request',hostId:'local',request:{id,method,params}});
  });true`);
  const rpc=(method,params={})=>evaluate(sessionId,`__coexistRequest(${JSON.stringify(method)},${JSON.stringify(params)})`);
  let status;
  for(let i=0;i<100;i++){try{status=await local('clawkit/status');if(status.ready)break}catch{}await delay(100)}
  assert.equal(status?.ready,true,'remote endpoint initialized by official desktop');
  const created=await rpc('thread/start',{cwd:home,ephemeral:true}); testThread=created.thread.id;
  const inventory=await rpc('mcpServerStatus/list',{threadId:testThread,serverName:'codex_app'});
  fs.writeFileSync(path.join(home,'inventory.json'),JSON.stringify(inventory,null,2));
  const server=inventory.data.find(s=>s.name==='codex_app');
  assert(server,'bundled MCP server');
  if(process.env.CLAWKIT_EXPECT_REJECTION==='1') {
    assert(server.toolsError?.includes('pipe closed'),'original signing rejection reproduced');
    const result={home,remoteReady:status.ready,originalFailureReproduced:true,toolsError:server.toolsError};
    fs.writeFileSync(path.join(process.env.CLAWKIT_TEST_RESULTS||home,'original-wrapper-result.json'),JSON.stringify(result,null,2));
    console.log(JSON.stringify(result,null,2));return;
  }
  assert(server.tools?.automation_update,'official automation tool loaded');
  const processes=execFileSync('/bin/ps',['-axo','pid,ppid,comm'],{encoding:'utf8'}).trim().split('\n').slice(1).map(l=>{
    const m=l.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);return m&&{pid:+m[1],ppid:+m[2],command:m[3]}
  }).filter(Boolean);
  const official=processes.find(p=>p.ppid===child.pid&&p.command.endsWith('/codex'));
  assert(official,'official engine directly parented by official app');
  const relay=processes.find(p=>p.ppid===official.pid&&p.command===adapter);
  assert(relay,'relay is engine child');
  const models=await local('model/list',{}); assert(Array.isArray(models.data),'remote RPC forwards');
  const result={home,appPid:child.pid,enginePid:official.pid,engineParent:official.ppid,
    relayPid:relay.pid,relayParent:relay.ppid,remoteReady:status.ready,
    officialTools:Object.keys(server.tools).length,automationToolPresent:true,remoteModelList:true,
    automationActionsVerified:false,
    thread:testThread,hostBytesUnmodified:true};
  const output=process.env.CLAWKIT_TEST_RESULTS||home;
  fs.writeFileSync(path.join(output,'official-coexist-result.json'),JSON.stringify(result,null,2));
  console.log(JSON.stringify(result,null,2));
})().catch(error=>{console.error(error);console.error('Evidence: '+home);process.exitCode=1}).finally(async()=>{
  // Only this explicitly created isolated application is stopped.
  child.kill('SIGTERM');
  await Promise.race([new Promise(r=>child.once('exit',r)),delay(5000)]);
  if(child.exitCode===null)child.kill('SIGKILL');
  process.exit(process.exitCode||0);
});
