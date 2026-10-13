function (scopeFactory, nj, open, close, clear, priority, pinned, recent, q, unread, latestReply) {
  // References come from the hash-verified, already loaded official modules.
  // Never evaluate source or method names received from the phone.
  const scope = () => {
    const root = document.querySelector('#root') || document.body.firstElementChild;
    const key = Object.keys(root || {}).find(k => k.startsWith('__reactContainer$'));
    const fiber = root?.[key];
    const queue = [fiber?.current || fiber], seen = new Set();
    while (queue.length && seen.size < 30000) {
      const f = queue.pop();
      if (!f || seen.has(f)) continue;
      seen.add(f);
      const chain = f.memoizedProps?.value;
      if (chain instanceof Map && chain.has(q.id)) return scopeFactory(q, chain, chain.get(q.id));
      queue.push(f.child, f.sibling);
    }
    throw Error('官方窗口状态尚未就绪');
  };
  globalThis.__clawkitActivity = async (action, threadId) => {
    if (action === 'markRead') {
      const m = await import('app://-/assets/app-shared-8bd9d0399038.js');
      const session = await m.fN.threadReadState.open(() => {});
      if (session.status !== 'ready') throw Error('官方已读服务尚未就绪');
      try {
        const result = await session.session.set({hostId:'local', threadId, hasUnreadTurn:false});
        if (result.status !== 'ok') throw Error('官方已读服务拒绝更新');
        return {read:!m.dQt.isUnread('local',threadId)};
      } finally { await session.session.unsubscribe(); }
    }
    const s = scope(), mode = s.get(nj)?.sidebarMode || 'codex';
    if (action === 'open') open(s, mode);
    if (action === 'clear') clear(s, mode);
    if (action === 'close') close(s);
    const state = s.get(nj);
    if (!state) return {version:1,open:false,priority:[],pinned:[],groups:[]};
    let unsupported = 0;
    const project = item => {
      const entry = item.threadEntry;
      if (item.kind !== 'task' || entry?.kind !== 'local' || (entry.hostId || 'local') !== 'local' || !entry.conversationId) {
        unsupported++; return null;
      }
      const id = entry.conversationId, summary = entry.summary || {};
      return {id, name:summary.title || summary.name || '', cwd:summary.cwd || '',
        createdAt:(summary.createdAt || 0)/1000, updatedAt:(summary.updatedAt || 0)/1000,
        status:item.attentionState, hasUnreadTurn:unread(s.get,entry.key),
        latestReply:s.get(latestReply,{hostId:'local',threadId:id}) || undefined};
    };
    const rows = items => (items || []).map(project).filter(Boolean);
    return {version:1,open:true,activatedAt:state.activatedAt,mode:state.sidebarMode,
      priority:rows(s.get(priority,state.sidebarMode)),pinned:rows(s.get(pinned,state.sidebarMode)),
      groups:s.get(recent,state.sidebarMode).map(group=>({dayStart:group.dayStart,rows:rows(group.items)})),
      unsupported,observedAt:Date.now()};
  };
  return true;
}
