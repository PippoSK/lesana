'use strict';

// Exercise the shipped browser script without a browser or npm dependencies.
// Run with: node --test tests/chat_recovery.test.cjs
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const script = readFileSync(join(__dirname, '../web/app.js'), 'utf8');
const html = readFileSync(join(__dirname, '../web/index.html'), 'utf8');
const pendingKey = 'lesana.pending-chat.v1';
const conversationId = '11111111-1111-4111-8111-111111111111';
const requestId = '22222222-2222-4222-8222-222222222222';
const question = 'Aká je lehota na podanie žaloby?';
const answer = 'Overená odpoveď zo servera.';
const answerPayload = { answer, sources: [], mode: 'agent', claims: [], tool_calls: [] };
const savedMessages = [
  { id: 1, role: 'user', content: question, sources: [], claims: [], tool_calls: [] },
  { id: 2, role: 'assistant', content: answer, sources: [], mode: 'agent', claims: [], tool_calls: [] },
];

class Events {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, listener, options = {}) {
    const entries = this.listeners.get(type) || [];
    entries.push({ listener, once: options.once });
    this.listeners.set(type, entries);
  }
  removeEventListener(type, listener) {
    this.listeners.set(type, (this.listeners.get(type) || []).filter(entry => entry.listener !== listener));
  }
  dispatchEvent(event) {
    event.preventDefault ||= () => {};
    for (const entry of [...(this.listeners.get(event.type) || [])]) {
      if (entry.once) this.removeEventListener(event.type, entry.listener);
      entry.listener(event);
    }
    return true;
  }
}

class Element extends Events {
  constructor(tagName = 'div') {
    super();
    Object.assign(this, { tagName: tagName.toUpperCase(), childNodes: [], parentNode: null, dataset: {}, attributes: {}, className: '', value: '', open: false, disabled: false, _text: '' });
    const classes = () => new Set(this.className.split(/\s+/).filter(Boolean));
    this.classList = {
      contains: name => classes().has(name),
      add: (...names) => { this.className = [...new Set([...classes(), ...names])].join(' '); },
      remove: (...names) => { this.className = [...classes()].filter(name => !names.includes(name)).join(' '); },
      toggle: (name, force) => {
        const include = force ?? !this.classList.contains(name);
        this.classList[include ? 'add' : 'remove'](name);
        return include;
      },
    };
  }
  get children() { return this.childNodes.filter(node => node.tagName !== '#TEXT'); }
  get textContent() { return this._text + this.childNodes.map(node => node.textContent).join(''); }
  set textContent(value) { this.replaceChildren(); this._text = String(value ?? ''); }
  get isConnected() { return !!this.parentNode; }
  append(...nodes) {
    for (let node of nodes) {
      if (typeof node === 'string') { const text = new Element('#text'); text.textContent = node; node = text; }
      node.remove(); node.parentNode = this; this.childNodes.push(node);
    }
  }
  replaceChildren(...nodes) {
    for (const child of this.childNodes) child.parentNode = null;
    this.childNodes = []; this._text = ''; this.append(...nodes);
  }
  remove() {
    if (this.parentNode) this.parentNode.childNodes = this.parentNode.childNodes.filter(node => node !== this);
    this.parentNode = null;
  }
  cloneNode(deep = false) {
    const clone = new Element(this.tagName);
    clone.className = this.className; clone._text = this._text;
    if (deep) clone.append(...this.childNodes.map(node => node.cloneNode(true)));
    return clone;
  }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name] ?? null; }
  querySelectorAll(selector) {
    const matches = node => {
      if (selector.startsWith('.')) return selector.slice(1).split('.').every(name => node.classList.contains(name));
      if (/^\[data-[\w-]+\]$/.test(selector)) return selector.slice(6, -1).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()) in node.dataset;
      return node.tagName.toLowerCase() === selector;
    };
    return this.childNodes.flatMap(node => [...(matches(node) ? [node] : []), ...node.querySelectorAll(selector)]);
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  showModal() { this.open = true; }
  close() { this.open = false; }
  focus() {}
  scrollIntoView() {}
  requestSubmit() { this.dispatchEvent({ type: 'submit' }); }
}

function storage(initial = {}) {
  const values = new Map(Object.entries(initial));
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, String(value)), removeItem: key => values.delete(key) };
}

async function flush() { for (let i = 0; i < 80; i++) await Promise.resolve(); }

function json(data, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => data };
}

function stream(...chunks) {
  let cursor = 0;
  const reader = {
    read: async () => {
      const chunk = chunks[cursor++];
      if (chunk instanceof Error) throw chunk;
      return chunk === undefined ? { done: true } : { done: false, value: new TextEncoder().encode(chunk) };
    },
    cancel: async () => {},
    releaseLock: () => {},
  };
  return { ok: true, status: 200, body: { getReader: () => reader } };
}

function frame(event, data) { return `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`; }

function hangingStream(signal) {
  return {
    ok: true, status: 200,
    body: { getReader: () => ({
      read: () => new Promise((resolve, reject) => {
        if (signal?.aborted) reject(signal.reason || new DOMException('Aborted', 'AbortError'));
        else signal?.addEventListener('abort', () => reject(signal.reason || new DOMException('Aborted', 'AbortError')), { once: true });
      }),
      cancel: async () => {}, releaseLock: () => {},
    }) },
  };
}

async function harness(options = {}) {
  let now = Date.parse('2026-10-23T10:00:00Z');
  let timerId = 0;
  const timers = new Map();
  const elements = new Map();
  for (const match of html.matchAll(/<([\w-]+)\b[^>]*\bid="([^"]+)"[^>]*>/g)) {
    const element = new Element(match[1]);
    element.id = match[2];
    element.className = /class="([^"]*)"/.exec(match[0])?.[1] || '';
    elements.set(element.id, element);
  }
  const document = new Events();
  Object.assign(document, {
    visibilityState: 'visible', hidden: false,
    getElementById: id => { assert.ok(elements.has(id), `Unknown DOM element ${id}`); return elements.get(id); },
    createElement: tag => new Element(tag),
    createElementNS: (_, tag) => new Element(tag),
    createTextNode: text => { const node = new Element('#text'); node.textContent = text; return node; },
    querySelectorAll: () => [],
  });
  const window = new Events();
  const sessionStorage = options.storage || storage();
  const requests = [];
  const context = vm.createContext({
    document, window, sessionStorage, localStorage: storage(), console,
    navigator: { onLine: true }, TextDecoder, TextEncoder, URL, URLSearchParams, FormData,
    AbortController, DOMException, Error, TypeError, crypto: { getRandomValues: bytes => bytes.fill(42), randomUUID: () => requestId },
    Date: class extends Date { constructor(...args) { super(...(args.length ? args : [now])); } static now() { return now; } },
    setTimeout: (callback, delay = 0) => { const id = ++timerId; timers.set(id, { at: now + delay, callback }); return id; },
    clearTimeout: id => timers.delete(id),
    confirm: () => true,
    fetch: async (path, config = {}) => {
      const request = { path, ...config, method: config.method || 'GET' };
      requests.push(request);
      if (options.fetch) {
        const response = await options.fetch(request);
        if (response !== undefined) return response;
      }
      if (path === '/api/corpus') return json({ versions: [], agent_enabled: true, model: 'test' });
      if (path === '/api/session') return json({});
      if (path === '/api/source-requests') return json([]);
      if (path === '/api/conversations') return json(request.method === 'POST' ? { id: conversationId } : []);
      if (path === `/api/conversations/${conversationId}/attachments`) return json([]);
      if (path === `/api/conversations/${conversationId}`) return json(options.messages || ((options.storage || requests.some(previous => /\/chat$/.test(previous.path))) ? savedMessages : []));
      throw new Error(`Unexpected fetch: ${request.method} ${path}`);
    },
  });
  Object.assign(window, { document, sessionStorage, navigator: context.navigator });
  vm.runInContext(script, context, { filename: 'web/app.js' });
  await flush();
  const app = {
    document, window, requests, storage: sessionStorage,
    node: id => elements.get(id),
    state: () => vm.runInContext('state', context),
    chatPosts: () => requests.filter(request => request.method === 'POST' && /\/chat$/.test(request.path)),
    statusGets: () => requests.filter(request => request.method === 'GET' && /\/chat\//.test(request.path)),
    assistants: () => elements.get('messages').children.filter(node => node.classList.contains('assistant')),
    users: () => elements.get('messages').children.filter(node => node.classList.contains('user')),
    upload: files => context.uploadFiles(files),
    loadConversation: conversation => context.loadConversation(conversation),
    removeAttachment: file => context.removeAttachment(file),
    async submit(value = question) { elements.get('question').value = value; elements.get('chat-form').requestSubmit(); await flush(); },
    async event(target, type, extra = {}) { target.dispatchEvent({ type, ...extra }); await flush(); },
    async advance(milliseconds) {
      const end = now + milliseconds;
      for (let iteration = 0; iteration < 2000; iteration++) {
        await flush();
        const next = [...timers.entries()].filter(([, timer]) => timer.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
        if (!next) { now = end; await flush(); return; }
        const [id, timer] = next; timers.delete(id); now = timer.at; timer.callback();
      }
      throw new Error('Timer loop did not settle');
    },
  };
  return app;
}

function assertCompleted(app) {
  assert.equal(app.state().busy, false);
  assert.equal(app.assistants().length, 1);
  assert.ok(app.assistants()[0].textContent.includes(answer));
  assert.equal(app.users().length, 1);
  assert.equal(app.chatPosts().length, 1, 'Recovery must never send the question twice');
  assert.equal(app.storage.getItem(pendingKey), null);
  assert.equal(app.node('error').textContent, '');
}

test('normal streamed answer is displayed once and clears the recovery marker', async () => {
  const app = await harness({ fetch: request => /\/chat$/.test(request.path) ? stream(frame('answer', answerPayload)) : undefined });
  await app.submit();
  assertCompleted(app);
  assert.ok(JSON.parse(app.chatPosts()[0].body).request_id, 'A stable request ID is sent with the question');
});

test('Safari Load failed resumes running work and loads the saved answer without another POST', async () => {
  let polls = 0;
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(frame('status', { text: 'Pracujem…' }), new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) return json(++polls === 1 ? { state: 'running', text: 'Pracujem…' } : { state: 'completed' });
  } });
  await app.submit();
  assert.equal(app.state().busy, true);
  assert.ok(app.storage.getItem(pendingKey));
  await app.advance(30_000);
  assertCompleted(app);
  assert.ok(polls >= 2);
});

for (const wakeEvent of ['visibilitychange', 'pageshow']) {
  test(`${wakeEvent} recovers a stalled stream after returning to the browser`, async () => {
    const app = await harness({ fetch: request => {
      if (/\/chat$/.test(request.path)) return hangingStream(request.signal);
      if (/\/chat\//.test(request.path)) return json({ state: 'completed' });
    } });
    await app.submit();
    assert.equal(app.state().busy, true);
    if (wakeEvent === 'visibilitychange') {
      app.document.visibilityState = 'hidden'; app.document.hidden = true;
      await app.event(app.document, wakeEvent);
      app.document.visibilityState = 'visible'; app.document.hidden = false;
      await app.event(app.document, wakeEvent);
    } else await app.event(app.window, wakeEvent, { persisted: true });
    await app.advance(100);
    assertCompleted(app);
    assert.ok(app.statusGets().length);
  });
}

test('reload restores an unfinished question from sessionStorage without sending it again', async () => {
  const saved = storage({ [pendingKey]: JSON.stringify({ conversation: conversationId, requestId, question, afterMessageId: 0, startedAt: Date.parse('2026-10-23T09:59:00Z') }) });
  const app = await harness({ storage: saved, fetch: request => /\/chat\//.test(request.path) ? json({ state: 'completed' }) : undefined });
  await app.advance(100);
  assert.equal(app.state().busy, false);
  assert.equal(app.assistants().length, 1);
  assert.equal(app.users().length, 1);
  assert.equal(app.chatPosts().length, 0);
  assert.equal(app.storage.getItem(pendingKey), null);
  assert.equal(app.state().conversation, conversationId);
});

test('terminal server error is shown without polling or resubmitting', async () => {
  const app = await harness({ fetch: request => /\/chat$/.test(request.path) ? stream(frame('error', { error: 'Model odmietol otázku.', code: 'invalid_request' })) : undefined });
  await app.submit();
  await app.advance(60_000);
  assert.equal(app.state().busy, false);
  assert.equal(app.node('error').textContent, 'Model odmietol otázku.');
  assert.equal(app.statusGets().length, 0);
  assert.equal(app.chatPosts().length, 1);
  assert.equal(app.storage.getItem(pendingKey), null);
});

test('authentication expiry preserves pending work until login succeeds', async () => {
  let authenticated = false;
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) return authenticated ? json({ state: 'completed' }) : json({ error: 'Prihláste sa.' }, 401);
  } });
  await app.submit();
  assert.equal(app.node('login-dialog').open, true);
  assert.ok(app.storage.getItem(pendingKey));
  authenticated = true;
  app.node('password').value = 'test-password';
  await app.event(app.node('login-form'), 'submit');
  await app.advance(100);
  assertCompleted(app);
  assert.equal(app.node('login-dialog').open, false);
});

test('temporary failures during recovery are retried without duplicating the question', async () => {
  let polls = 0;
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) {
      if (++polls < 3) throw new TypeError('Load failed');
      return json({ state: 'completed' });
    }
  } });
  await app.submit();
  await app.advance(60_000);
  assertCompleted(app);
  assert.equal(polls, 3);
});

test('an answer followed by a lost stream cannot duplicate the displayed answer', async () => {
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(frame('answer', answerPayload), new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) return json({ state: 'completed' });
  } });
  await app.submit();
  await app.advance(60_000);
  assertCompleted(app);
});

test('a stream that silently stops responding is recovered after its timeout', async () => {
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return hangingStream(request.signal);
    if (/\/chat\//.test(request.path)) return json({ state: 'completed' });
  } });
  await app.submit();
  await app.advance(180_000);
  assertCompleted(app);
});

test('a recovery response whose JSON body hangs is bounded by a timeout', async () => {
  let polls = 0;
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) {
      if (++polls === 1) return { ok: true, status: 200, json: () => new Promise((resolve, reject) => request.signal?.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')), { once: true })) };
      return json({ state: 'completed' });
    }
  } });
  await app.submit();
  await app.advance(180_000);
  assertCompleted(app);
  assert.equal(polls, 2);
});

test('expired run status still restores its durable saved answer', async () => {
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) return json({ error: 'Neznáma požiadavka.' }, 404);
  } });
  await app.submit();
  assertCompleted(app);
});

test('a failed background run reports its error and stops recovery', async () => {
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) return json({ state: 'failed', error: 'Model nie je dostupný.', code: 'model_error' });
  } });
  await app.submit();
  await app.advance(60_000);
  assert.equal(app.state().busy, false);
  assert.equal(app.node('error').textContent, 'Model nie je dostupný.');
  assert.equal(app.node('question').value, question);
  assert.equal(app.statusGets().length, 1);
  assert.equal(app.chatPosts().length, 1);
  assert.equal(app.storage.getItem(pendingKey), null);
});

test('an earlier identical question cannot be mistaken for the unfinished answer', async () => {
  const app = await harness({ messages: savedMessages, fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) return json({ error: 'Neznáma požiadavka.' }, 404);
  } });
  await app.submit();
  await app.advance(60_000);
  assert.equal(app.state().busy, false);
  assert.ok(app.node('error').textContent.includes('nepodarilo obnoviť'));
  assert.equal(app.node('question').value, question);
  assert.equal(app.assistants().length, 1, 'The previously saved answer remains intact');
  assert.equal(app.users().length, 1, 'Only the optimistic question is removed');
  assert.equal(app.chatPosts().length, 1);
  assert.equal(app.statusGets().length, 3);
});

test('offline recovery waits for connectivity and the online event wakes it immediately', async () => {
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return hangingStream(request.signal);
    if (/\/chat\//.test(request.path)) return json({ state: 'completed' });
  } });
  await app.submit();
  app.window.navigator.onLine = false;
  await app.advance(40_000);
  assert.equal(app.statusGets().length, 0);
  assert.equal(app.state().busy, true);
  app.window.navigator.onLine = true;
  await app.event(app.window, 'online');
  assertCompleted(app);
});

test('transient HTTP failures during recovery can be retried safely', async () => {
  let polls = 0;
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return stream(new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) {
      if (++polls === 1) return json({ error: 'Dočasne nedostupné.' }, 503);
      if (polls === 2) return json({ error: 'Skúste neskôr.' }, 429);
      return json({ state: 'completed' });
    }
  } });
  await app.submit();
  await app.advance(30_000);
  assertCompleted(app);
  assert.equal(polls, 3);
});

test('failed sidebar refresh cannot turn a delivered answer into a failed question', async () => {
  let delivered = false;
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) { delivered = true; return stream(frame('answer', answerPayload)); }
    if (request.path === '/api/conversations' && request.method === 'GET' && delivered) throw new TypeError('Load failed');
  } });
  await app.submit();
  assertCompleted(app);
  assert.equal(app.node('question').value, '');
});

test('restoring a running question does not duplicate its already saved user message', async () => {
  const saved = storage({ [pendingKey]: JSON.stringify({ conversation: conversationId, requestId, question, afterMessageId: 0, startedAt: Date.parse('2026-10-23T09:59:00Z') }) });
  const app = await harness({ storage: saved, messages: [savedMessages[0]], fetch: request => /\/chat\//.test(request.path) ? json({ state: 'running', text: 'Pracujem…' }) : undefined });
  assert.equal(app.state().busy, true);
  assert.equal(app.users().length, 1);
  assert.equal(app.chatPosts().length, 0);
  assert.ok(app.storage.getItem(pendingKey));
});

test('HTTP 409 reconciles the existing server job instead of resending the question', async () => {
  const app = await harness({ fetch: request => {
    if (/\/chat$/.test(request.path)) return json({ error: 'Odpoveď sa už pripravuje.' }, 409);
    if (/\/chat\//.test(request.path)) return json({ state: 'completed' });
  } });
  await app.submit();
  assertCompleted(app);
  assert.equal(app.statusGets().length, 1);
});

test('login into a new session preserves the question and permits a fresh conversation', async () => {
  let authenticated = false;
  const newConversation = '33333333-3333-4333-8333-333333333333';
  const app = await harness({ fetch: request => {
    if (request.path === '/api/conversations' && request.method === 'POST' && authenticated) return json({ id: newConversation });
    if (request.path === `/api/conversations/${newConversation}`) return json([]);
    if (request.path === `/api/conversations/${newConversation}/chat`) return stream(frame('answer', answerPayload));
    if (request.path === `/api/conversations/${conversationId}` && authenticated) return json({ error: 'Rozhovor neexistuje.' }, 404);
    if (/\/chat$/.test(request.path)) return stream(new TypeError('Load failed'));
    if (/\/chat\//.test(request.path)) return authenticated ? json({ error: 'Rozhovor neexistuje.' }, 404) : json({ error: 'Prihláste sa.' }, 401);
  } });
  await app.submit();
  assert.equal(app.node('login-dialog').open, true);
  assert.ok(app.storage.getItem(pendingKey));
  authenticated = true;
  await app.event(app.node('login-form'), 'submit');
  await app.advance(100);
  assert.equal(app.state().busy, false);
  assert.equal(app.state().conversation, null, 'The inaccessible conversation must not trap future submissions');
  assert.equal(app.node('question').value, question);
  assert.equal(app.storage.getItem(pendingKey), null);
  assert.equal(app.chatPosts().length, 1);
  await app.submit();
  assert.equal(app.state().conversation, newConversation);
  assert.equal(app.state().busy, false);
  assert.equal(app.assistants().length, 1);
  assert.equal(app.users().length, 1);
  assert.equal(app.chatPosts().length, 2, 'A second POST is allowed only after the user explicitly submits again');
  assert.equal(app.node('error').textContent, '');
});

const attachmentId = '44444444-4444-4444-8444-444444444444';
const attachment = { id: attachmentId, filename: 'Zmluva.txt', content_type: 'text/plain', size_bytes: 21, text_chars: 21 };
const attachmentUrl = `/api/conversations/${conversationId}/attachments`;

test('upload creates a conversation, lets the browser set multipart boundary and retains the question', async () => {
  const app = await harness({ fetch: request => {
    if (request.path === attachmentUrl && request.method === 'POST') {
      assert.ok(request.body instanceof FormData);
      assert.equal(request.headers['Content-Type'], undefined);
      assert.equal(request.body.get('file').name, attachment.filename);
      return json(attachment, 201);
    }
  } });
  app.node('question').value = question;
  await app.upload([new File(['Obsah pracovnej zmluvy'], attachment.filename)]);
  assert.equal(app.state().conversation, conversationId);
  assert.equal(app.node('question').value, question);
  assert.equal(app.state().attachments[0].id, attachmentId);
  assert.equal(app.node('attachments-panel').classList.contains('hidden'), false);
  assert.equal(app.node('attachments').querySelector('a').download, attachment.filename);
  assert.equal(app.state().attachmentBusy, false);
  assert.equal(app.chatPosts().length, 0);
});

test('invalid file batches are rejected before creating or uploading anything', async () => {
  const app = await harness();
  const initialRequests = app.requests.length;
  for (const files of [
    [new File(['payload'], 'program.exe')], [new File([], 'empty.txt')],
    [{ name: 'large.pdf', size: 10 * 1024 * 1024 + 1 }],
    Array.from({ length: 6 }, (_, i) => new File(['text'], `${i}.txt`)),
  ]) {
    await app.upload(files);
    assert.equal(app.requests.length, initialRequests);
    assert.ok(app.node('error').textContent);
    assert.equal(app.state().conversation, null);
  }
});

test('upload locks chat and conversation switching until the file has been processed', async () => {
  let completeUpload;
  const app = await harness({ fetch: request => {
    if (request.path === attachmentUrl && request.method === 'POST') return new Promise(resolve => { completeUpload = resolve; });
  } });
  const upload = app.upload([new File(['text'], attachment.filename)]);
  await flush();
  assert.equal(app.state().attachmentBusy, true);
  assert.equal(app.node('send').disabled, true);
  await app.submit();
  await app.event(app.node('new-chat'), 'click');
  assert.equal(app.state().conversation, conversationId);
  assert.equal(app.chatPosts().length, 0);
  completeUpload(json(attachment, 201)); await upload;
  assert.equal(app.node('send').disabled, false);
});

test('a lost upload response reconciles server attachments without repeating the upload', async () => {
  const app = await harness({ fetch: request => {
    if (request.path !== attachmentUrl) return;
    if (request.method === 'POST') throw new TypeError('Load failed');
    return json([attachment]);
  } });
  await app.upload([new File(['text'], attachment.filename)]);
  assert.equal(app.state().attachments[0].id, attachmentId);
  assert.equal(app.requests.filter(r => r.path === attachmentUrl && r.method === 'POST').length, 1);
  assert.equal(app.node('error').textContent.includes('Spojenie'), true);
  assert.equal(app.state().attachmentBusy, false);
});

test('partial upload failure keeps successful files visible and stops the batch', async () => {
  let posted = 0;
  const app = await harness({ fetch: request => {
    if (request.path !== attachmentUrl) return;
    if (request.method === 'GET') return json([attachment]);
    return ++posted === 1 ? json(attachment, 201) : json({ error: 'Súbor sa nedá prečítať.' }, 400);
  } });
  await app.upload(['Zmluva.txt', 'Sken.pdf', 'Dalsi.txt'].map(name => new File(['text'], name)));
  assert.equal(posted, 2);
  assert.equal(app.state().attachments.length, 1);
  assert.equal(app.node('error').textContent, 'Súbor sa nedá prečítať.');
  assert.equal(app.state().attachmentBusy, false);
});

test('opening a conversation restores its files and removal updates the available context', async () => {
  const app = await harness({ fetch: request => {
    if (request.path === attachmentUrl) return json([attachment]);
    if (request.path === `${attachmentUrl}/${attachmentId}` && request.method === 'DELETE') return json(null, 204);
  } });
  await app.loadConversation({ id: conversationId, title: 'Zmluva' });
  assert.equal(app.state().attachments.length, 1);
  await app.removeAttachment(attachment);
  assert.equal(app.state().attachments.length, 0);
  assert.equal(app.node('attachments-panel').classList.contains('hidden'), true);
  assert.equal(app.node('send').disabled, false);
});

test('file names render as text and starting a new conversation clears its attachment list', async () => {
  const file = { ...attachment, filename: '<img src=x onerror=alert(1)>.txt' };
  const app = await harness({ fetch: request => request.path === attachmentUrl ? json([file]) : undefined });
  await app.loadConversation({ id: conversationId, title: 'Zmluva' });
  assert.equal(app.node('attachments').querySelector('a').textContent, file.filename);
  assert.equal(app.node('attachments').querySelector('img'), null);
  await app.event(app.node('new-chat'), 'click');
  assert.equal(app.state().conversation, null);
  assert.equal(app.state().attachments.length, 0);
});

test('conversation deletion blocks uploads until the deleted conversation is cleared', async () => {
  let completeDelete;
  let deleted = false;
  const app = await harness({ fetch: request => {
    if (request.path === '/api/conversations' && request.method === 'GET') return json(deleted ? [] : [{ id: conversationId, title: 'Zmluva' }]);
    if (request.path === `/api/conversations/${conversationId}` && request.method === 'DELETE') {
      return new Promise(resolve => { completeDelete = () => { deleted = true; resolve(json(null, 204)); }; });
    }
  } });
  await app.loadConversation({ id: conversationId, title: 'Zmluva' });
  await app.event(app.node('history').children[0].children[1], 'click');
  assert.equal(app.state().attachmentBusy, true);
  await app.upload([new File(['text'], 'Zmluva.txt')]);
  assert.equal(app.requests.filter(r => r.path === attachmentUrl && r.method === 'POST').length, 0);
  completeDelete(); await flush();
  assert.equal(app.state().conversation, null);
  assert.equal(app.state().attachmentBusy, false);
});

test('dropping files onto the composer uploads them without navigating away', async () => {
  const app = await harness({ fetch: request => request.path === attachmentUrl && request.method === 'POST' ? json(attachment, 201) : undefined });
  let prevented = false;
  await app.event(app.node('chat-form'), 'drop', {
    dataTransfer: { types: ['Files'], files: [new File(['text'], attachment.filename)] },
    preventDefault: () => { prevented = true; },
  });
  assert.equal(prevented, true);
  assert.equal(app.state().attachments[0].id, attachmentId);
});

test('an inaccessible upload conversation is cleared without losing the typed question', async () => {
  const app = await harness({ fetch: request => request.path === attachmentUrl && request.method === 'POST'
    ? json({ error: 'Rozhovor sa nenašiel.' }, 404) : undefined });
  app.node('question').value = question;
  await app.upload([new File(['text'], attachment.filename)]);
  assert.equal(app.state().conversation, null);
  assert.equal(app.node('question').value, question);
  assert.equal(app.state().attachments.length, 0);
  assert.equal(app.node('send').disabled, false);
});
