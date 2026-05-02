(() => {
  function visible(el) {
    if (!el || !el.getBoundingClientRect) return false;
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden' && s.opacity !== '0';
  }
  function byText(text) {
    if (!text) return null;
    const needle = String(text).toLowerCase();
    const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT);
    let n, fallback = null;
    while ((n = walker.nextNode())) {
      if ((n.nodeValue || '').toLowerCase().includes(needle)) {
        const el = n.parentElement;
        if (visible(el)) return el;
        fallback ||= el;
      }
    }
    return fallback;
  }
  function resolve(params = {}) {
    if (params.selector) {
      try { const el = document.querySelector(params.selector); if (el) return el; } catch {}
    }
    if (params.text) { const el = byText(params.text); if (el) return el; }
    if (Number.isFinite(params.x) && Number.isFinite(params.y)) return document.elementFromPoint(params.x, params.y);
    return null;
  }
  function summary(el) {
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { tag: el.tagName, id: el.id || null, classes: String(el.className || '') || null, text: (el.innerText || el.value || el.ariaLabel || '').slice(0, 200), rect: { x: r.x, y: r.y, width: r.width, height: r.height } };
  }
  function selector(el) {
    if (!el) return null;
    if (el.id) return `#${CSS.escape(el.id)}`;
    if (el.name) return `[name="${CSS.escape(el.name)}"]`;
    const parts=[]; let cur=el;
    while (cur && cur.nodeType===1 && cur !== document.body && parts.length < 4) {
      let p=cur.tagName.toLowerCase();
      const cls=String(cur.className||'').split(/\s+/).find(c=>c && !c.includes(':'));
      if (cls) p += `.${CSS.escape(cls)}`;
      parts.unshift(p); cur=cur.parentElement;
    }
    return parts.join(' > ');
  }
  function interactables() {
    const els = [...document.querySelectorAll('a,button,input:not([type=hidden]),textarea,select,[role=button],[onclick],[contenteditable=true]')].filter(visible).slice(0, 300);
    return els.map((el, i) => ({ index: i, type: el.tagName.toLowerCase(), text: (el.innerText || el.value || el.placeholder || el.ariaLabel || el.href || '').trim().slice(0, 120), selector: selector(el), href: el.href || null, rect: summary(el).rect }));
  }
  async function click(params) {
    const el = resolve(params); if (!el) throw new Error('Element not found');
    el.scrollIntoView?.({block:'center', inline:'center'}); el.focus?.({preventScroll:true});
    for (const type of ['mouseover','mousedown','mouseup','click']) el.dispatchEvent(new MouseEvent(type, {bubbles:true,cancelable:true,view:window}));
    el.click?.(); return { clicked: true, element: summary(el) };
  }
  async function type(params) {
    const el = resolve(params); if (!el) throw new Error('Element not found');
    const text = String(params.text ?? '');
    el.scrollIntoView?.({block:'center', inline:'center'}); el.focus?.({preventScroll:true});
    if (el.isContentEditable) el.textContent = params.append ? (el.textContent + text) : text;
    else if ('value' in el) el.value = params.append ? (el.value + text) : text;
    else throw new Error('Target not editable');
    el.dispatchEvent(new InputEvent('input', {bubbles:true, inputType:'insertText', data:text}));
    el.dispatchEvent(new Event('change', {bubbles:true}));
    if (params.submit) el.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', code:'Enter', bubbles:true}));
    return { typed: true, length: text.length, element: summary(el) };
  }
  async function fillForm(params) {
    const results=[];
    for (const f of params.fields || []) results.push(await type(f));
    if (params.submit) document.querySelector('form')?.requestSubmit?.();
    return { filled: results.length, results };
  }
  async function waitFor(params) {
    const timeout = params.timeoutMs || params.timeout || 10000;
    const start = Date.now();
    while (Date.now() - start < timeout) {
      if (params.text && document.body.innerText.toLowerCase().includes(String(params.text).toLowerCase())) return { found: true, type: 'text' };
      if (params.selector && document.querySelector(params.selector)) return { found: true, type: 'selector' };
      await new Promise(r => setTimeout(r, 200));
    }
    throw new Error('waitFor timeout');
  }
  chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    (async () => {
      const p = msg.params || {};
      switch (msg.action) {
        case 'click': return click(p);
        case 'type': return type(p);
        case 'fillForm': return fillForm(p);
        case 'waitFor': return waitFor(p);
        case 'getContent': {
          const fmt=p.format||'annotated';
          if (fmt==='html') return { html: document.documentElement.outerHTML, title: document.title, url: location.href };
          const text=(document.body?.innerText || document.documentElement.innerText || '');
          if (fmt==='text') return { text, title: document.title, url: location.href };
          return { text, interactables: interactables(), title: document.title, url: location.href };
        }
        case 'getInteractables': return { interactables: interactables(), title: document.title, url: location.href };
        case 'eval': {
          if (p.allowUnsafe !== true) throw new Error('eval requires allowUnsafe:true');
          return { result: await (0, eval)(p.script) };
        }
        case 'scroll': {
          if (p.position === 'top') scrollTo(0,0); else if (p.position === 'bottom') scrollTo(0, document.body.scrollHeight); else scrollBy(p.x||0, p.y||p.dy||600);
          return { x: scrollX, y: scrollY };
        }
        default: throw new Error('Unknown content action: '+msg.action);
      }
    })().then(r => sendResponse({ok:true,result:r})).catch(e => sendResponse({ok:false,error:String(e.message||e)}));
    return true;
  });
})();
