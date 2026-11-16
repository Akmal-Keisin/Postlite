import assert from 'node:assert/strict';
import { cmd, evaluate, close, screenshot, reload } from './tier1-cdp.mjs';

const results = [];
const check = async (name, expression) => {
  const value = await evaluate(`(async()=>(${expression}))()`);
  assert.equal(value, true, `${name}: ${JSON.stringify(value)}`);
  results.push(name);
  console.log(`PASS ${name}`);
};
const click = selector => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
const settle = () => evaluate('__verify.flushWorkspace().then(()=>true)');

try {
  await reload();
  await evaluate(`(() => {
    window.__runtimeErrors=[]; window.addEventListener('error', e=>__runtimeErrors.push(e.message));
    const v=__verify;
    v.state.collections=[]; v.state.environments=[]; v.state.tabs=[]; v.state.activeTabId=null;
    v.state.settings=v.normalizeSettings(); v.applyAppSettings(); v.persist();
  })()`);

  await check('examples default to an empty array',
    `JSON.stringify(__verify.newRequest({}).examples)==='[]'`);
  await check('example names are trimmed and capped at 80 chars',
    `__verify.normExamples([{name:'  '+('x'.repeat(100))+'  '}])[0].name.length===80 && __verify.normExamples([{}])[0].name==='Example'`);
  await check('malformed examples are dropped and fields normalized',
    `(() => {const ex=__verify.normExamples([null,7,{status:'201',headers:[{key:'X',value:3}],body:'ok',elapsedMs:'12'}]);return ex.length===1 && ex[0].status===201 && ex[0].headers[0].value==='3' && ex[0].body==='ok' && ex[0].elapsedMs===12 && typeof ex[0].id==='string';})()`);
  console.log(`\n${results.length} checks passed`);
} finally {
  await close();
}
