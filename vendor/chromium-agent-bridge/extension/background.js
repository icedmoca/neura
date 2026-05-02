const WS_URL = 'ws://127.0.0.1:8767/chromium-extension';
let ws = null;
let reconnectTimer = null;
let cachedActiveTabId = null;

function connect() {
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return;
  ws = new WebSocket(WS_URL);
  ws.onopen = () => { ws.send(JSON.stringify({type:'hello', role:'extension', browser:'chromium', version:'0.1.0'})); updateBadge(''); };
  ws.onmessage = ev => { try { const m = JSON.parse(ev.data); if (m && m.type && !m.action) return; handle(m); } catch (e) { send({ok:false,error:String(e.message||e)}); } };
  ws.onclose = () => { updateBadge('!'); scheduleReconnect(); };
  ws.onerror = () => { updateBadge('!'); };
}
function scheduleReconnect(){ if (!reconnectTimer) reconnectTimer=setTimeout(()=>{reconnectTimer=null; connect();}, 1000); }
function send(msg){ if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg)); }
function updateBadge(text){ chrome.action?.setBadgeText({text}).catch?.(()=>{}); }
chrome.runtime.onStartup.addListener(connect); chrome.runtime.onInstalled.addListener(() => { chrome.alarms.create('cab-keepalive', { periodInMinutes: 0.25 }); connect(); }); chrome.alarms.onAlarm.addListener(() => { if (ws && ws.readyState === WebSocket.OPEN) send({type:'heartbeat', time:Date.now()}); else connect(); }); chrome.alarms.create('cab-keepalive', { periodInMinutes: 0.25 }); setInterval(() => { if (ws && ws.readyState === WebSocket.OPEN) send({type:'heartbeat', time:Date.now()}); else connect(); }, 10000); connect();

async function activeTab() { const tabs = await chrome.tabs.query({active:true, currentWindow:true}); return tabs[0] || (await chrome.tabs.query({active:true}))[0]; }
async function resolveTabId(params={}) { if (Number.isInteger(params.tabId)) return params.tabId; if (cachedActiveTabId) return cachedActiveTabId; const t=await activeTab(); if (!t) throw new Error('No active tab'); return t.id; }
async function waitComplete(tabId, timeout=15000){
  const t=await chrome.tabs.get(tabId); if (t.status === 'complete') return;
  await new Promise((resolve,reject)=>{ const timer=setTimeout(()=>{cleanup();reject(new Error('Timed out waiting for page load'));}, timeout); function onUpdated(id, info){ if(id===tabId && info.status==='complete'){cleanup(); resolve();} } function cleanup(){clearTimeout(timer); chrome.tabs.onUpdated.removeListener(onUpdated);} chrome.tabs.onUpdated.addListener(onUpdated); });
}
async function sendToContent(action, params={}) { const tabId=await resolveTabId(params); const r=await chrome.tabs.sendMessage(tabId, {action, params}); if (!r?.ok) throw new Error(r?.error || 'content script failed'); return r.result; }
async function listTabs(){ const wins=await chrome.windows.getAll({populate:true}); const active=(await activeTab())?.id ?? null; return {activeTabId: active, totalTabs: wins.flatMap(w=>w.tabs||[]).length, windows: wins.map(w=>({windowId:w.id, focused:w.focused, tabs:(w.tabs||[]).map(t=>({tabId:t.id,index:t.index,active:t.active,title:t.title,url:t.url,windowId:t.windowId}))}))}; }
async function dispatch(action, params={}) {
  switch(action){
    case 'ping': return {pong:true,time:Date.now(), browser:'chromium'};
    case 'listTabs': return listTabs();
    case 'getActiveTab': return activeTab();
    case 'setActiveTab': { const tabId=await resolveTabId(params); await chrome.tabs.update(tabId,{active:true}); const tab=await chrome.tabs.get(tabId); if (params.focus !== false) await chrome.windows.update(tab.windowId,{focused:true}); cachedActiveTabId=tabId; return tab; }
    case 'newSession': { const tab=await chrome.tabs.create({url:params.url||'about:blank', active:params.active!==false}); cachedActiveTabId=tab.id; if(params.wait!==false) await waitComplete(tab.id, params.timeoutMs); return tab; }
    case 'navigate': { const tabId=params.newTab ? (await chrome.tabs.create({url:params.url, active:true})).id : await resolveTabId(params); await chrome.tabs.update(tabId,{url:params.url, active:true}); cachedActiveTabId=tabId; if(params.wait!==false) await waitComplete(tabId, params.timeoutMs); return chrome.tabs.get(tabId); }
    case 'closeTab': { const tabId=await resolveTabId(params); await chrome.tabs.remove(tabId); if(cachedActiveTabId===tabId) cachedActiveTabId=null; return {closed:true, tabId}; }
    case 'screenshot': { const tabId=await resolveTabId(params); const tab=await chrome.tabs.get(tabId); await chrome.windows.update(tab.windowId,{focused:true}); await chrome.tabs.update(tabId,{active:true}); const dataUrl=await chrome.tabs.captureVisibleTab(tab.windowId,{format:params.format||'png'}); return {tabId,dataUrl}; }
    case 'getContent': case 'getInteractables': case 'click': case 'type': case 'fillForm': case 'waitFor': case 'eval': case 'scroll': return sendToContent(action, params);
    default: throw new Error('Unknown action: '+action);
  }
}
async function handle(msg){ const id=msg.id; try { const result=await dispatch(msg.action, msg.params||{}); send({id,ok:true,result}); } catch(e){ send({id,ok:false,error:String(e.message||e)}); } }
