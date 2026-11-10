import { writeFileSync } from 'node:fs';
const targets = await (await fetch('http://127.0.0.1:9235/json/list')).json();
const page = targets.find(t => t.type === 'page' && t.url.startsWith('http://127.0.0.1:8100'));
if (!page) throw new Error('Isolated verification page not found');
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise(resolve => socket.onopen = resolve);
const pending = new Map();
let next = 0;
socket.onmessage = ({ data }) => {
  const msg = JSON.parse(data);
  if (!msg.id) return;
  const item = pending.get(msg.id);
  if (!item) return;
  pending.delete(msg.id);
  msg.error ? item.reject(new Error(msg.error.message)) : item.resolve(msg.result);
};
export const cmd = (method, params = {}) => new Promise((resolve, reject) => {
  const id = ++next;
  pending.set(id, { resolve, reject });
  socket.send(JSON.stringify({ id, method, params }));
});
export const evaluate = async expression => {
  const result = await cmd('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? JSON.stringify(result.exceptionDetails));
  return result.result.value;
};
export const reload = async () => {
  await cmd('Page.enable');
  const loaded = new Promise(resolve => {
    const handler = ({data}) => { if(JSON.parse(data).method==='Page.loadEventFired') {socket.removeEventListener('message',handler);resolve();} };
    socket.addEventListener('message',handler);
  });
  await cmd('Page.navigate',{url:`http://127.0.0.1:8100/?run=${Date.now()}`});
  await loaded;
  await evaluate(`new Promise((resolve,reject)=>{const start=Date.now();const timer=setInterval(()=>{if(window.__verify){clearInterval(timer);resolve(true);}else if(Date.now()-start>5000){clearInterval(timer);reject(new Error('startup failed'));}},30);})`);
};
export const close = () => socket.close();
export const screenshot = async name => {
  const { data } = await cmd('Page.captureScreenshot', { format: 'png' });
  writeFileSync(new URL(name, import.meta.url), Buffer.from(data, 'base64'));
};
if (process.argv[1]?.endsWith('tier1-cdp.mjs')) {
  try { console.log(JSON.stringify(await evaluate(process.argv[2] || '({ready:!!window.__verify,error:document.querySelector("#error")?.textContent})'), null, 2)); }
  finally { close(); }
}
