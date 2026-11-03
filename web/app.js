'use strict';
const $ = (id) => document.getElementById(id);
const state = { conversation: null, busy: false, attachments: [], attachmentBusy: false, corpus: null, history: [], currentSources: [], sourceRequests: [], requestLoading: false, requestSubmitting: false, requestPollTimer: null, corpusRefreshNeeded: false, pending: null, chatController: null, recoveryWake: null };
const pendingChatKey = 'lesana.pending-chat.v1';
const emptySources = $('sources').cloneNode(true);

function el(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}
function error(message) { $('error').textContent = message || ''; $('error').classList.toggle('hidden', !message); }
async function api(path, options = {}) {
  const { timeout = 15000, ...requestOptions } = options;
  const multipart = typeof FormData !== 'undefined' && requestOptions.body instanceof FormData;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeout);
  try {
    const response = await fetch(path, { credentials: 'same-origin', ...requestOptions, signal: controller.signal,
      headers: { ...(multipart ? {} : { 'Content-Type': 'application/json' }), ...requestOptions.headers } });
    if (!response.ok) {
      const body = await response.json().catch(() => ({}));
      if (response.status === 401 && path !== '/api/session') showLogin();
      throw Object.assign(new Error(body.error || `Požiadavka zlyhala (${response.status}).`), { status: response.status });
    }
    return response.status === 204 ? null : await response.json();
  } catch (failure) {
    if (failure.name === 'TypeError' || failure.name === 'AbortError') throw connectionError();
    throw failure;
  } finally { clearTimeout(timer); }
}
function connectionError() {
  return Object.assign(new Error('Spojenie so serverom sa prerušilo. Skontrolujte pripojenie na internet.'), { connectionLost: true });
}
function showLogin() { if (!$('login-dialog').open) $('login-dialog').showModal(); }
function formatDate(date) { return date ? new Intl.DateTimeFormat('sk-SK').format(new Date(`${date}T12:00:00`)) : '—'; }
function safeLink(url, label) {
  const anchor = el('a', '', label);
  let parsed;
  try { parsed = new URL(url); } catch { return el('span', '', label); }
  if (parsed.protocol !== 'https:' || !['static.slov-lex.sk', 'www.slov-lex.sk', 'eur-lex.europa.eu', 'nsud.sk', 'www.nsud.sk', 'rozhodnuti.nsoud.cz'].includes(parsed.hostname)) return el('span', '', label);
  anchor.href = parsed.href; anchor.target = '_blank'; anchor.rel = 'noopener noreferrer';
  return anchor;
}

function sourceRetrieval(source) {
  const methods = {
    fulltext: { label: 'Fulltext', icon: 'M10 3a7 7 0 1 0 0 14a7 7 0 0 0 0-14M15 15l6 6M7 8h6M7 11h4', description: 'Fulltext: zhoda slov alebo podobného znenia vo vyhľadávanom texte.' },
    embedding: { label: 'Embedding', icon: 'M7 6a2 2 0 1 0-4 0a2 2 0 0 0 4 0M21 6a2 2 0 1 0-4 0a2 2 0 0 0 4 0M14 18a2 2 0 1 0-4 0a2 2 0 0 0 4 0M7 6h10M6 8l5 8M18 8l-5 8', description: 'Embedding: podobnosť významu otázky a ustanovenia pri sémantickom vyhľadávaní.' },
    exact: { label: 'Presný odkaz', icon: 'M10 13a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-2 2M14 11a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l2-2', description: 'Presný odkaz: ustanovenie načítané podľa konkrétneho paragrafu, článku, odseku alebo písmena.' },
    related: { label: 'Súvisiace ustanovenie', icon: 'M10 13a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-2 2M14 11a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l2-2', description: 'Automaticky doplnený celý paragraf, článok alebo ustanovenie z odkazu v zdroji.' },
    unknown: { label: 'Spôsob nezaznamenaný', icon: 'M12 3a9 9 0 1 0 0 18a9 9 0 0 0 0-18M9.5 9a2.5 2.5 0 1 1 4 2l-1.5 1v2M12 17h.01', description: 'Pri tomto podklade sa spôsob vyhľadania neuložil. Staršie odpovede tento údaj neobsahujú.' },
  };
  const recorded = Array.isArray(source.retrieval_methods) ? source.retrieval_methods : [];
  const keys = ['fulltext', 'embedding', 'exact', 'related'].filter(key => recorded.includes(key));
  if (!keys.length) keys.push('unknown');
  const details = el('details', 'source-retrieval');
  const summary = el('summary');
  summary.title = keys.map(key => methods[key].description).join('\n');
  summary.setAttribute('aria-label', `Spôsob vyhľadania: ${keys.map(key => methods[key].label).join(' a ')}. Podrobnosti`);
  for (const key of keys) {
    const method = methods[key];
    const badge = el('span', `retrieval-badge retrieval-${key}`);
    const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    icon.setAttribute('viewBox', '0 0 24 24');
    icon.setAttribute('aria-hidden', 'true');
    icon.setAttribute('focusable', 'false');
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute('d', method.icon); icon.append(path);
    badge.append(icon, document.createTextNode(method.label)); summary.append(badge);
  }
  const chevron = el('span', 'retrieval-chevron', '⌄'); chevron.setAttribute('aria-hidden', 'true'); summary.append(chevron);
  details.append(summary);
  for (const key of keys) details.append(el('p', 'retrieval-description', methods[key].description));
  if (keys.length > 1) details.append(el('p', 'retrieval-description', 'Podklad sa našiel viacerými spôsobmi.'));
  return details;
}

function isSupremeCourtJudgment(judgment) { return !!(judgment?.collection || judgment?.ecli?.startsWith('ECLI:SK:NSSR:')); }
function isCzechSupremeCourtJudgment(judgment) { return !!judgment?.ecli?.startsWith('ECLI:CZ:NS:'); }
function judgmentIdentity(judgment) { return [judgment.court, judgment.ecli].filter(Boolean).join(' · '); }
function collectionDescription(collection) {
  return `Zbierka stanovísk a rozhodnutí · čiastka ${collection.issue} · R ${collection.number}/${collection.year} · strany PDF ${collection.first_page}–${collection.last_page}${collection.ocr ? ' · text rozpoznaný pomocou OCR; citované pasáže overte v PDF' : ''}.`;
}

function sourceCard(source) {
  const card = el('article', 'source-card'); card.dataset.sourceId = source.id;
  const judgment = source.judgment;
  const collection = judgment?.collection;
  const supremeCourt = isSupremeCourtJudgment(judgment);
  const czechCourt = isCzechSupremeCourtJudgment(judgment);
  const treaty = source.treaty;
  const regulation = source.regulation;
  card.append(el('div', 'source-id', `ZDROJ N${source.id} · ${czechCourt ? "JUDIKATÚRA · NS ČR · PODPORNÁ ARGUMENTÁCIA" : supremeCourt ? "JUDIKATÚRA · NAJVYŠŠÍ SÚD SR" : judgment ? "JUDIKATÚRA · EUR-LEX" : treaty ? "DOHOVOR · EUR-LEX · EN" : regulation ? "NARIADENIE EÚ · EUR-LEX" : "SLOV-LEX"}`), el('h3', '', source.citation));
  if (judgment) card.append(el('p', 'small', judgmentIdentity(judgment)), el('p', 'small', judgment.legal_context));
  if (collection) card.append(el('p', 'small', collectionDescription(collection)));
  if (treaty) card.append(el('p', 'small', treaty.legal_context));
  if (regulation) card.append(el('p', 'small', `CELEX ${regulation.celex} · pôvodné slovenské znenie`), el('p', 'small', regulation.legal_context));
  if (source.heading) card.append(el('p', 'small', source.heading));
  card.append(el('div', 'source-text', source.text));
  if (source.context && source.context !== source.text) {
    const details = el('details'); details.append(el('summary', '', collection ? 'Zobraziť text uverejnený v zbierke' : judgment ? 'Zobraziť celé rozhodnutie' : regulation ? 'Zobraziť kontext a poznámky' : 'Zobraziť celý nadradený odsek'), el('pre', '', source.context)); card.append(details);
  }
  const links = el('div', 'source-links');
  links.append(safeLink(source.source_url, czechCourt ? 'Rozhodnutie na nsoud.cz (CS) ↗' : collection ? 'Zbierka na nsud.sk ↗' : supremeCourt ? 'Rozhodnutie na nsud.sk ↗' : judgment ? 'Rozsudok na EUR-Lexe ↗' : treaty ? 'Dohovor na EUR-Lexe ↗' : regulation ? 'Nariadenie na EUR-Lexe ↗' : 'Slov-Lex ↗'), ...(source.pdf_url ? [safeLink(source.pdf_url, collection ? 'PDF čiastky zbierky ↗' : supremeCourt ? 'PDF rozhodnutia ↗' : judgment ? 'PDF rozsudku ↗' : treaty ? 'PDF konsolidácie (EN) ↗' : regulation ? 'PDF nariadenia ↗' : 'Záväzné PDF ↗')] : []));
  card.append(links, el('p', 'source-date', judgment ? `Rozhodnutie z ${formatDate(judgment.decided_on)} · zdroj overený ${formatDate(source.verified_on)}` : treaty ? `Konsolidácia uverejnená ${formatDate(treaty.published_on)} · angličtina · zdroj overený ${formatDate(source.verified_on)}` : regulation ? `Uverejnené ${formatDate(regulation.published_on)} · pôvodné znenie · zdroj overený ${formatDate(source.verified_on)}` : `Účinnosť od ${formatDate(source.valid_from)}${source.valid_to ? ` do ${formatDate(source.valid_to)}` : ''} · overené ${formatDate(source.verified_on)}`));
  card.append(sourceRetrieval(source));
  return card;
}
function showSources(sources, highlight) {
  state.currentSources = sources;
  $('sources').replaceChildren(...sources.map(sourceCard));
  $('source-count').textContent = sources.length;
  if (!sources.length) $('sources').replaceChildren(...emptySources.cloneNode(true).childNodes);
  if (highlight) {
    $('sources-panel').classList.add('open'); $('sources-panel').classList.remove('closed');
    const card = [...$('sources').children].find(node => node.dataset.sourceId === String(highlight));
    if (card) { card.classList.add('highlight'); card.scrollIntoView({ block: 'nearest' }); }
  }
}
function renderAnswerText(container, content, sources) {
  // Build bold text and citations with DOM nodes; model HTML stays plain text.
  const parts = content.split(/(\*\*[\s\S]+?\*\*|\[N\d+\])/g);
  for (const part of parts) {
    if (part.length > 4 && part.startsWith('**') && part.endsWith('**')) {
      const strong = el('strong');
      renderAnswerText(strong, part.slice(2, -2), sources);
      container.append(strong);
      continue;
    }
    const match = /^\[N(\d+)\]$/.exec(part);
    const source = match && sources.find(s => String(s.id) === match[1]);
    if (source) {
      const button = el('button', 'citation-button', `${source.judgment ? '⚖' : '§'} ${sources.indexOf(source) + 1}`);
      button.title = source.citation; button.setAttribute('aria-label', `Zobraziť zdroj: ${source.citation}`);
      button.addEventListener('click', () => showSources(sources, source.id)); container.append(button);
    } else container.append(document.createTextNode(part));
  }
}
function addMessage(role, content, sources = [], mode = '', date = '', claims = [], architecture = null, toolCalls = []) {
  $('welcome').classList.add('hidden');
  const message = el('article', `message ${role}`);
  if (role === 'user') message.textContent = content;
  else {
    const label = el('div', 'message-label'); label.append(el('span', 'brand-mark', 'l.'), el('span', '', 'Lesana'));
    const body = el('div', 'message-body'); renderAnswerText(body, content, sources); message.append(label, body);
    if (claims.length) {
      const evidence = el('details', 'claim-evidence');
      evidence.append(el('summary', '', 'Tvrdenia a podkladové pasáže'));
      for (const claim of claims) {
        const block = el('section', 'claim-evidence-item');
        block.append(el('p', '', claim.text));
        for (const item of claim.evidence) {
          const source = sources.find(s => s.id === item.source_id);
          block.append(el('blockquote', '', item.quote));
          if (source) {
            const button = el('button', 'citation-button', source.citation);
            button.addEventListener('click', () => showSources(sources, source.id));
            block.append(button);
          }
        }
        evidence.append(block);
      }
      message.append(evidence);
    }
    const modes = { research: 'Režim rešerše · model nie je pripojený', research_fallback: 'Podklady na rešerš · bez overeného záveru', agent: 'Odpoveď s citáciami · overte vecnú správnosť', agent_partial: 'Čiastočná odpoveď · overte vecnú správnosť', no_coverage: 'Nedostupná časová verzia', no_sources: 'Nedostatok podkladov', clarification: 'Potrebné doplnenie okolností' };
    const architectureName = mode === 'research' ? '' : ({ pipeline: 'RAG pipeline', agentic: 'Agentic RAG' }[architecture] || '');
    message.append(el('div', 'message-meta', `${modes[mode] || ''}${architectureName ? ` · ${architectureName}` : ''}${date ? ` · Stav k ${formatDate(date)}` : ''}`));
    if (toolCalls.length) {
      const trace = el('details', 'tool-trace');
      trace.append(el('summary', '', `Použité nástroje (${toolCalls.length})`));
      for (const call of toolCalls) trace.append(el('p', '', `${toolLabel(call.name)}${call.as_of ? ` · stav k ${formatDate(call.as_of)}` : ''} · ${call.outcome === 'ok' ? `${call.source_count} podkladov` : 'neúspešné volanie'}`));
      message.append(trace);
    }
    if (sources.length) {
      const button = el('button', 'message-sources', `§  Zobraziť zdroje (${sources.length})`);
      button.addEventListener('click', () => { showSources(sources); $('sources-panel').classList.add('open'); $('sources-panel').classList.remove('closed'); });
      message.append(button);
    }
  }
  $('messages').append(message); $('scroll-area').scrollTop = $('scroll-area').scrollHeight;
  return message;
}
async function refreshHistory() {
  state.history = await api('/api/conversations'); $('history-count').textContent = state.history.length;
  $('history').replaceChildren();
  if (!state.history.length) $('history').append(el('p', 'muted small', 'Tu nájdete svoje rozhovory.'));
  for (const conversation of state.history) {
    const row = el('div', `history-row${state.conversation === conversation.id ? ' active' : ''}`);
    const open = el('button', '', conversation.title); open.title = conversation.title;
    open.addEventListener('click', () => loadConversation(conversation).catch(e => error(e.message)));
    const remove = el('button', 'history-delete', '×'); remove.setAttribute('aria-label', `Vymazať rozhovor ${conversation.title}`);
    remove.addEventListener('click', async () => {
      if (state.busy || state.attachmentBusy || !confirm('Vymazať tento rozhovor aj s jeho správami a súbormi?')) return;
      let removedCurrent = false;
      setAttachmentBusy(true);
      try {
        await api(`/api/conversations/${conversation.id}`, { method: 'DELETE' });
        removedCurrent = state.conversation === conversation.id;
        await refreshHistory();
      }
      catch (e) { error(e.message); }
      finally { setAttachmentBusy(false); if (removedCurrent) newChat(); }
    });
    row.append(open, remove); $('history').append(row);
  }
}
async function loadConversation(conversation) {
  if (state.busy || state.attachmentBusy) return;
  setAttachmentBusy(true);
  try {
    const [messages, attachments] = await Promise.all([
      api(`/api/conversations/${conversation.id}`), api(`/api/conversations/${conversation.id}/attachments`),
    ]);
    state.conversation = conversation.id; state.attachments = attachments;
    $('chat-title').textContent = conversation.title;
    renderMessages(messages); renderAttachments(); attachmentStatus('');
    $('sidebar').classList.remove('open'); await refreshHistory();
  } finally { setAttachmentBusy(false); }
}
function renderMessages(messages) {
  $('messages').replaceChildren();
  $('welcome').classList.toggle('hidden', messages.length > 0); showSources([]); error('');
  for (const message of messages) addMessage(message.role, message.content, message.sources, message.mode, message.as_of, message.claims, message.architecture, message.tool_calls);
  const last = messages.filter(m => m.role === 'assistant').at(-1);
  if (last) showSources(last.sources);
}
function newChat() {
  if (state.busy || state.attachmentBusy) return;
  state.conversation = null; $('messages').replaceChildren(); $('welcome').classList.remove('hidden'); $('chat-title').textContent = 'Nový rozhovor';
  state.attachments = []; renderAttachments(); attachmentStatus('');
  $('question').value = ''; error(''); showSources([]); $('sidebar').classList.remove('open'); $('question').focus();
  for (const row of $('history').children) row.classList.remove('active');
}
function setBusy(busy) {
  state.busy = busy; updateComposerControls();
  $('progress').classList.toggle('hidden', !busy);
}
function updateComposerControls() {
  const locked = state.busy || state.attachmentBusy;
  $('send').disabled = locked; $('question').disabled = state.busy;
  $('attach-files').disabled = locked || state.attachments.length >= 5;
  $('file-input').disabled = locked;
  for (const button of $('attachments').querySelectorAll('button')) button.disabled = locked;
}
function setAttachmentBusy(busy) {
  state.attachmentBusy = busy; updateComposerControls();
  $('attachments-panel').setAttribute('aria-busy', String(busy));
}
function attachmentStatus(message) {
  $('attachment-status').textContent = message;
  $('attachment-status').classList.toggle('hidden', !message);
}
function attachmentPath(id, conversation = state.conversation) {
  return `/api/conversations/${encodeURIComponent(conversation)}/attachments${id ? `/${encodeURIComponent(id)}` : ''}`;
}
function fileSize(bytes) {
  return bytes < 1024 * 1024 ? `${Math.max(1, Math.ceil(bytes / 1024))} kB`
    : `${new Intl.NumberFormat('sk-SK', { maximumFractionDigits: 1 }).format(bytes / (1024 * 1024))} MB`;
}
function renderAttachments() {
  $('attachments-panel').classList.toggle('hidden', state.attachments.length === 0);
  $('attachment-count').textContent = `${state.attachments.length}/5`;
  $('attachments').replaceChildren(...state.attachments.map(file => {
    const row = el('li', 'attachment');
    const link = el('a', 'attachment-name', file.filename);
    link.href = attachmentPath(file.id); link.download = file.filename;
    link.title = `Stiahnuť ${file.filename}`;
    const remove = el('button', 'attachment-remove', '×'); remove.type = 'button';
    remove.setAttribute('aria-label', `Odstrániť súbor ${file.filename}`);
    remove.addEventListener('click', () => removeAttachment(file));
    row.append(link, el('span', 'attachment-size', fileSize(file.size_bytes)), remove);
    return row;
  }));
  updateComposerControls();
}
async function refreshAttachments(conversation = state.conversation) {
  if (!conversation) return;
  const files = await api(attachmentPath(null, conversation));
  if (state.conversation === conversation) { state.attachments = files; renderAttachments(); }
}
async function uploadFiles(selectedFiles) {
  if (state.busy || state.attachmentBusy) return;
  const files = Array.from(selectedFiles || []); if (!files.length) return;
  $('file-input').value = '';
  error('');
  if (files.length + state.attachments.length > 5) { error('V rozhovore môžete mať najviac 5 súborov. Najprv niektorý odstráňte.'); return; }
  for (const file of files) {
    if (!/\.(pdf|docx|txt|md)$/i.test(file.name)) { error(`${file.name}: podporované sú PDF, DOCX, TXT a MD.`); return; }
    if (!file.size || file.size > 10 * 1024 * 1024) { error(`${file.name}: súbor musí byť neprázdny a mať najviac 10 MB.`); return; }
  }
  setAttachmentBusy(true);
  let uploaded = 0;
  try {
    if (!state.conversation) {
      const conversation = await api('/api/conversations', { method: 'POST' });
      state.conversation = conversation.id;
    }
    // Sequential requests keep partial successes visible and avoid duplicate retries.
    for (const file of files) {
      attachmentStatus(`Nahrávam a čítam ${file.name} (${uploaded + 1}/${files.length})…`);
      const body = new FormData(); body.append('file', file);
      const attachment = await api(attachmentPath(), { method: 'POST', body, timeout: 120000 });
      state.attachments.push(attachment); uploaded++; renderAttachments();
    }
    attachmentStatus('Súbory sú pripravené. Napíšte, s čím vám má Lesana pomôcť.');
  } catch (failure) {
    // A lost response can hide a successful upload; reload before a manual retry.
    if (failure.status === 403 || failure.status === 404) resetUnavailableConversation();
    else await refreshAttachments().catch(() => {});
    error(failure.message); attachmentStatus(uploaded ? `Úspešne nahrané súbory: ${uploaded}.` : '');
  } finally {
    setAttachmentBusy(false); $('file-input').value = ''; $('question').focus();
    await refreshHistory().catch(() => {});
  }
}
async function removeAttachment(file) {
  if (state.busy || state.attachmentBusy) return;
  setAttachmentBusy(true); error(''); attachmentStatus(`Odstraňujem ${file.filename}…`);
  try {
    await api(attachmentPath(file.id), { method: 'DELETE' });
    state.attachments = state.attachments.filter(item => item.id !== file.id); renderAttachments();
    attachmentStatus('Súbor bol odstránený. Pri ďalších otázkach sa už nepoužije.');
  } catch (failure) { error(failure.message); attachmentStatus(''); await refreshAttachments().catch(() => {}); }
  finally { setAttachmentBusy(false); }
}
function savePendingChat(pending) {
  state.pending = pending;
  // A tab reload must not lose the question while the server is still working.
  try {
    if (pending) sessionStorage.setItem(pendingChatKey, JSON.stringify(pending));
    else sessionStorage.removeItem(pendingChatKey);
  } catch { /* Recovery in the current page still works when storage is disabled. */ }
}
function readPendingChat() {
  try {
    const pending = JSON.parse(sessionStorage.getItem(pendingChatKey));
    const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
    if (pending && uuid.test(pending.conversation) && uuid.test(pending.requestId)
      && typeof pending.question === 'string' && pending.question.length <= 12000
      && Number.isSafeInteger(pending.afterMessageId) && pending.afterMessageId >= 0
      && Number.isFinite(pending.startedAt)) return pending;
  } catch { /* Ignore invalid or unavailable tab storage. */ }
  return null;
}
function chatRequestId() {
  // getRandomValues also works on local HTTP deployments without randomUUID.
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 15) | 64; bytes[8] = (bytes[8] & 63) | 128;
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
function recoveryDelay(milliseconds = 2500) {
  return new Promise(resolve => {
    const wake = () => { clearTimeout(timer); if (state.recoveryWake === wake) state.recoveryWake = null; resolve(); };
    const timer = setTimeout(wake, milliseconds);
    state.recoveryWake = wake;
  });
}
function resumeChatConnection() {
  if (document.visibilityState === 'hidden' || navigator.onLine === false) return;
  // A suspended browser can leave a reader pending forever without rejecting it.
  // Reconcile with the existing server job; never send the question a second time.
  state.chatController?.abort();
  state.recoveryWake?.();
}
async function streamChat(pending) {
  const controller = new AbortController(); state.chatController = controller;
  let reader; let timer;
  const heartbeat = () => { clearTimeout(timer); timer = setTimeout(() => controller.abort(), 35000); };
  heartbeat();
  try {
    const response = await fetch(`/api/conversations/${pending.conversation}/chat`, { method: 'POST', credentials: 'same-origin', signal: controller.signal,
      headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ message: pending.question, request_id: pending.requestId }) });
    if (!response.ok) {
      if (response.status === 409) throw connectionError();
      const body = await response.json().catch(() => ({}));
      if (response.status === 401) showLogin();
      throw Object.assign(new Error(body.error || 'Odoslanie zlyhalo.'), { definitive: true });
    }
    reader = response.body.getReader(); const decoder = new TextDecoder(); let buffer = '';
    while (true) {
      const { value, done } = await reader.read(); heartbeat();
      buffer += decoder.decode(value, { stream: !done });
      buffer = buffer.replace(/\r\n/g, '\n');
      let boundary;
      while ((boundary = buffer.indexOf('\n\n')) >= 0) {
        const frame = buffer.slice(0, boundary); buffer = buffer.slice(boundary + 2);
        const lines = frame.split('\n'); const name = lines.find(line => line.startsWith('event:'))?.slice(6).trim();
        const raw = lines.filter(line => line.startsWith('data:')).map(line => line.slice(5).trimStart()).join('\n');
        if (!raw) continue; const payload = JSON.parse(raw);
        if (name === 'status') $('progress-text').textContent = payload.text;
        if (name === 'error') throw Object.assign(new Error(payload.error), { definitive: true });
        if (name === 'tool') $('progress-text').textContent = `${toolLabel(payload.name)} · ${payload.source_count} podkladov`;
        if (name === 'answer') return payload;
      }
      if (done) throw connectionError();
    }
  } catch (failure) {
    if (failure.definitive) throw failure;
    throw connectionError();
  } finally {
    clearTimeout(timer);
    reader?.cancel().catch(() => {});
    if (state.chatController === controller) state.chatController = null;
  }
}
function hasRecoveredAnswer(messages, pending) {
  const newer = messages.filter(message => message.id > pending.afterMessageId);
  return newer.some((message, index) => message.role === 'user' && message.content === pending.question
    && newer[index + 1]?.role === 'assistant');
}
async function recoverChat(pending) {
  $('progress-text').textContent = 'Obnovujem spojenie s rozpracovanou odpoveďou…';
  let missing = 0;
  while (true) {
    if (document.visibilityState === 'hidden' || navigator.onLine === false || $('login-dialog').open) {
      if (navigator.onLine === false) $('progress-text').textContent = 'Čakám na pripojenie. Po návrate načítam odpoveď…';
      await recoveryDelay(); continue;
    }
    try {
      let run;
      try { run = await api(`/api/conversations/${pending.conversation}/chat/${pending.requestId}`); }
      catch (failure) { if (failure.status !== 404) throw failure; }
      if (run?.state === 'failed') throw new Error(run.error);
      if (run?.state === 'running') {
        missing = 0; $('progress-text').textContent = run.text || 'Lesana ďalej pripravuje odpoveď…';
      } else {
        // History is durable even if a server restart or retention expiry lost the run status.
        const messages = await api(`/api/conversations/${pending.conversation}`);
        if (hasRecoveredAnswer(messages, pending)) return messages;
        if (++missing >= 3) throw new Error('Odpoveď sa nepodarilo obnoviť. Otázka zostala pripravená na opätovné odoslanie.');
      }
    } catch (failure) {
      if (!failure.connectionLost && failure.status !== 401 && failure.status !== 429 && !(failure.status >= 500)) throw failure;
      $('progress-text').textContent = failure.status === 401
        ? 'Na obnovenie odpovede sa prihláste.' : 'Spojenie sa obnovuje. Vaša otázka sa znova neodosiela…';
    }
    await recoveryDelay();
  }
}
async function finishChat(pending, receive) {
  let completed = false;
  try {
    const result = await receive();
    if (Array.isArray(result)) renderMessages(result);
    else {
      addMessage('assistant', result.answer, result.sources, result.mode, result.as_of, result.claims, result.architecture, result.tool_calls);
      showSources(result.sources);
    }
    completed = true; error(''); $('chat-title').textContent = pending.question.slice(0, 65);
    const optimistic = $('messages').querySelector('[data-pending-chat]');
    if (optimistic) delete optimistic.dataset.pendingChat;
  } catch (failure) {
    error(failure.message); $('question').value = pending.question;
    if (failure.status === 403 || failure.status === 404) resetUnavailableConversation();
    // Only remove the optimistic question; stored history remains intact.
    const optimistic = $('messages').querySelector('[data-pending-chat]'); optimistic?.remove();
  } finally {
    savePendingChat(null); setBusy(false);
    if (document.visibilityState !== 'hidden') $('question').focus();
  }
  // A sidebar refresh failure must never turn a delivered answer into a failed turn.
  if (completed) await refreshHistory().catch(() => {});
}
function resetUnavailableConversation() {
  state.conversation = null; $('messages').replaceChildren(); showSources([]);
  state.attachments = []; renderAttachments(); attachmentStatus('');
  $('welcome').classList.remove('hidden'); $('chat-title').textContent = 'Nový rozhovor';
}
async function restorePendingChat() {
  if (state.busy) return;
  const pending = readPendingChat(); if (!pending) return;
  state.conversation = pending.conversation; savePendingChat(pending); setBusy(true);
  $('chat-title').textContent = pending.question.slice(0, 65);
  try {
    const messages = await api(`/api/conversations/${pending.conversation}`); renderMessages(messages);
    await refreshAttachments(pending.conversation).catch(() => {});
    if (!messages.some(message => message.id > pending.afterMessageId && message.role === 'user' && message.content === pending.question)) {
      addMessage('user', pending.question).dataset.pendingChat = 'true';
    }
  } catch (failure) {
    if (failure.status === 403 || failure.status === 404) {
      savePendingChat(null); setBusy(false); resetUnavailableConversation(); $('question').value = pending.question; error(failure.message); return;
    }
  }
  await finishChat(pending, () => recoverChat(pending));
}
async function submit(event) {
  event.preventDefault(); if (state.busy || state.attachmentBusy) return;
  const question = $('question').value.trim(); if (!question) return;
  error(''); attachmentStatus(''); setBusy(true); $('progress-text').textContent = 'Pripravujem rozhovor…';
  try {
    if (!state.conversation) { const conversation = await api('/api/conversations', { method: 'POST' }); state.conversation = conversation.id; }
    const messages = await api(`/api/conversations/${state.conversation}`);
    renderMessages(messages);
    const pending = { conversation: state.conversation, requestId: chatRequestId(), question,
      afterMessageId: messages.at(-1)?.id || 0, startedAt: Date.now() };
    savePendingChat(pending);
    addMessage('user', question).dataset.pendingChat = 'true'; $('question').value = '';
    await finishChat(pending, async () => {
      try { return await streamChat(pending); }
      catch (failure) { if (!failure.connectionLost) throw failure; return recoverChat(pending); }
    });
  } catch (failure) { error(failure.message); setBusy(false); $('question').value = question; }
}

function renderCorpus(corpus) {
  const judgments = corpus.versions.filter(v => v.judgment).length;
  const regulations = corpus.versions.filter(v => v.regulation).length;
  const statutes = corpus.versions.length - judgments - regulations;
  $('library-count').textContent = `${statutes} verzií predpisov · ${judgments} ${judgments === 1 ? 'záznam judikatúry' : judgments < 5 ? 'záznamy judikatúry' : 'záznamov judikatúry'}${regulations ? ` · ${regulations} ${regulations === 1 ? 'nariadenie EÚ' : regulations < 5 ? 'nariadenia EÚ' : 'nariadení EÚ'}` : ''}`;
  $('mode-label').textContent = corpus.agent_enabled ? 'Model pripojený' : 'Režim rešerše';
  $('verified-date').textContent = 'Dátum overenia je uvedený pri každom zdroji';
  $('corpus-details').replaceChildren();
  for (const version of corpus.versions) {
    const judgment = version.judgment;
    const collection = judgment?.collection;
    const supremeCourt = isSupremeCourtJudgment(judgment);
    const czechCourt = isCzechSupremeCourtJudgment(judgment);
    const treaty = version.treaty;
    const regulation = version.regulation;
    const block = el('section'); block.append(el('h3', '', `${version.title} · ${version.number}`));
    block.append(el('p', '', judgment
      ? `${judgmentIdentity(judgment)} · ${collection ? 'rozhodnutie / stanovisko' : 'rozhodnutie'} z ${formatDate(judgment.decided_on)}. Zdroj overený ${formatDate(version.verified_on)}.`
      : treaty ? `Konsolidácia uverejnená ${formatDate(treaty.published_on)} · angličtina (EN). Zdroj overený ${formatDate(version.verified_on)}.`
      : regulation ? `Nariadenie EÚ · CELEX ${regulation.celex} · uverejnené ${formatDate(regulation.published_on)}. Pôvodné slovenské znenie; zdroj overený ${formatDate(version.verified_on)}.`
      : `Časová verzia účinná od ${formatDate(version.valid_from)}${version.valid_to ? ` do ${formatDate(version.valid_to)}` : ''}. Zdroj overený ${formatDate(version.verified_on)}. K dispozícii sú len znenia uvedené v knižnici.`));
    if (judgment) block.append(el('p', '', judgment.legal_context));
    if (collection) block.append(el('p', '', collectionDescription(collection)));
    if (treaty) block.append(el('p', '', treaty.legal_context));
    if (regulation) block.append(el('p', '', regulation.legal_context));
    const grid = el('div', 'corpus-stat-grid');
    const counts = czechCourt ? [['textových jednotiek', version.nodes], ['výrok', 1]] : collection ? [['strán publikovaného textu', judgment.expected_pages]] : supremeCourt ? [['strán PDF', judgment.expected_pages], ['výrok', 1]] : judgment ? [['bodov rozsudku', version.points], ['výrok', 1]] : treaty ? [['článkov dohovoru', treaty.expected_convention_articles], ['článkov pripojeného protokolu', treaty.expected_protocol_articles], ['článkov protokolu o výklade', treaty.expected_interpretation_articles]] : regulation ? [['článkov nariadenia', version.articles]] : [['paragrafov', version.sections], ...(version.articles ? [['článkov', version.articles]] : []), ['odsekov', version.paragraphs], ['písmen', version.letters]];
    for (const [name, value] of counts) {
      const stat = el('div', 'corpus-stat'); stat.append(el('strong', '', value), document.createTextNode(name)); grid.append(stat);
    }
    block.append(grid, el('p', '', czechCourt
      ? `Pôvodné české znenie s výrokom a odôvodnením. Používa sa iba na podpornú argumentáciu. Vyhľadávajte podľa spisovej značky alebo ECLI, napríklad „${version.number} odôvodnenie“. Úryvky nemajú vymyslené čísla bodov ani strany PDF.`
      : collection
      ? `Text uverejnený v zbierke môže byť výňatkom rozhodnutia. Vyhľadávajte podľa označenia R, spisovej značky, právnej vety alebo pôvodnej strany PDF, napríklad „R ${collection.number}/${collection.year} právna veta“.`
      : supremeCourt
      ? `Úplné rozhodnutie vrátane výroku. Vyhľadávajte podľa spisovej značky, ECLI alebo strany PDF, napríklad „${version.number} strana 2“. Overenie zdroja nezahŕňa kontrolu neskoršej judikatúry.`
      : judgment
      ? `Úplné slovenské znenie rozsudku vrátane výroku. Vyhľadávajte podľa čísla veci, mena, ECLI alebo bodu, napríklad „C-464/01 bod 39“. Overenie zdroja nezahŕňa kontrolu neskoršej judikatúry.`
      : treaty ? `Vyhľadávajte napríklad „čl. 13 BD“ alebo „čl. 13 protokolu o výklade BD“. Znenie zahŕňa dohovor, oba protokoly, poznámky a vyhlásenia. Bibliografické tabuľky sú dostupné v PDF. Slovenské vysvetlenie nenahrádza pôvodné anglické znenie.`
      : regulation ? `Vyhľadávajte podľa CELEX alebo článku a čísla nariadenia, napríklad „čl. 1 CELEX:${regulation.celex}“. Dátum uverejnenia nie je dátumom začatia uplatňovania. Neskoršie zmeny sa automaticky nedopĺňajú.`
      : `Celkovo ${version.nodes} uzlov vrátane základných princípov, častí, hláv, dielov, príloh a vnorených bodov. HTML slúži na vyhľadávanie, právne záväzné znenie je v PDF.`));
    const links = el('div', 'source-links'); links.append(safeLink(version.source_url, czechCourt ? 'Rozhodnutie na nsoud.cz (CS) ↗' : collection ? 'Zbierka na nsud.sk ↗' : supremeCourt ? 'Rozhodnutie na nsud.sk ↗' : judgment ? 'Rozsudok na EUR-Lexe ↗' : treaty ? 'Dohovor na EUR-Lexe ↗' : regulation ? 'Nariadenie na EUR-Lexe ↗' : 'Znenie na Slov-Lexe ↗'), ...(version.pdf_url ? [safeLink(version.pdf_url, collection ? 'PDF čiastky zbierky ↗' : supremeCourt ? 'PDF rozhodnutia ↗' : judgment ? 'PDF rozsudku ↗' : treaty ? 'PDF konsolidácie (EN) ↗' : regulation ? 'PDF nariadenia ↗' : 'Záväzné PDF ↗')] : []));
    block.append(links, el('p', 'checksum', `SHA-256: ${version.sha256}`)); $('corpus-details').append(block);
  }
  $('corpus-details').append(el('p', '', corpus.agent_enabled ? `Pripojený model: ${corpus.model}. Otázka, história tohto rozhovoru, text priložených súborov a vybrané právne zdroje sa odosielajú na nakonfigurovaný modelový server.` : 'Model zatiaľ nie je pripojený. Vyhľadávanie, čítanie zdrojov, súbory a história rozhovorov fungujú aj bez neho. Na rozbor priložených súborov musí správca pripojiť model.'));
}
function requestFeedback(message, failed = false) {
  $('source-request-feedback').textContent = message;
  $('source-request-feedback').classList.toggle('hidden', !message);
  $('source-request-feedback').classList.toggle('request-error', failed);
}
function updateSourceRequestKind() {
  const kind = $('source-request-kind').value;
  const judgment = kind === 'judgment';
  const collection = kind === 'supreme_court_collection';
  const supremeCourt = kind === 'supreme_court_judgment';
  const czechCourt = kind === 'czech_supreme_court_judgment';
  const regulation = kind === 'eu_regulation';
  $('source-request-label').textContent = czechCourt ? 'Odkaz na rozhodnutie Najvyššieho súdu ČR' : collection ? 'Odkaz na PDF celej čiastky zbierky' : supremeCourt ? 'Odkaz na PDF rozhodnutia Najvyššieho súdu SR' : judgment || regulation ? 'CELEX alebo odkaz na EUR-Lex' : 'Číslo predpisu alebo odkaz na Slov-Lex';
  $('source-request-reference').placeholder = czechCourt ? 'https://rozhodnuti.nsoud.cz/Judikatura/judikatura_ns.nsf/WebSearch/…?openDocument' : supremeCourt || collection ? 'https://www.nsud.sk/data/att/…pdf' : judgment ? '62001CJ0464 alebo https://eur-lex.europa.eu/…' : regulation ? '32012R1215 alebo https://eur-lex.europa.eu/…' : '311/2001 alebo https://static.slov-lex.sk/…';
  $('source-request-help').textContent = czechCourt
    ? 'Vložte odkaz na konkrétne rozhodnutie z databázy rozhodnuti.nsoud.cz; podporované sú aj staršie odkazy nsoud.cz/Judikatura/…. Pridá sa rozsudok alebo uznesenie s ECLI, výrokom a odôvodnením v češtine. V slovenskom prípade slúži iba na podpornú argumentáciu po porovnaní právnej úpravy a okolností.'
    : collection
    ? 'Zadajte priamy PDF odkaz na jednu čiastku Zbierky stanovísk a rozhodnutí z nsud.sk. Lesana pridá všetky rozhodnutia a stanoviská s označením R, právnou vetou a stranami PDF. Staršie skeny sa spracujú pomocou OCR; import môže trvať niekoľko minút. Zbierka môže obsahovať iba výňatky rozhodnutí.'
    : supremeCourt
    ? 'Zadajte priamy odkaz na textové PDF rozhodnutia z www.nsud.sk/data/att/…pdf. Importujú sa rozsudky a uznesenia s ECLI, dátumom, výrokom, odôvodnením a poučením. Odkaz na čiastku zbierky sa rozpozná automaticky a pridá jej obsah.'
    : judgment
    ? 'Zadajte CELEX rozsudku (napr. 62001CJ0464) alebo oficiálny HTML odkaz z eur-lex.europa.eu. Podporované sú rozsudky v slovenčine s číslovanými bodmi a výrokom; nepodporovaný formát oznámime.'
    : regulation ? 'Zadajte CELEX nariadenia (napr. 32012R1215) alebo slovenský HTML odkaz na EUR-Lex. Importuje sa pôvodné slovenské znenie s článkami. Konsolidované znenia a neskoršie zmeny sa automaticky nedopĺňajú.'
    : 'Zadajte číslo/rok pre aktuálne znenie (napr. 311/2001) alebo HTML odkaz na konkrétne časové znenie zo static.slov-lex.sk. Importuje sa len vybraná verzia.';
}
function renderSourceRequests() {
  const labels = { queued: 'Čaká na spracovanie', running: 'Pridáva sa do databázy', completed: 'Pridané do knižnice', failed: 'Pridanie zlyhalo' };
  const rows = state.sourceRequests.map(request => {
    const status = Object.hasOwn(labels, request.status) ? request.status : 'queued';
    const row = el('article', `source-request request-${status}`);
    const heading = el('div', 'source-request-heading');
    heading.append(el('strong', '', request.title || request.reference), el('span', 'request-status', labels[status]));
    row.append(heading);
    if (request.title) row.append(el('p', 'request-reference', request.reference));
    if (request.message) row.append(el('p', 'request-message', request.message));
    if (status === 'completed' && request.imported_count != null) row.append(el('p', 'request-result', `Počet pridaných záznamov: ${request.imported_count}${request.kind === 'supreme_court_collection' || request.imported_count > 1 ? ' (rozhodnutia a stanoviská zo zbierky)' : ''}.`));
    if (status === 'completed') row.append(el('p', 'request-result', request.node_count != null
      ? `Dostupných textových jednotiek: ${request.node_count}. Zdroj môžete vyhľadať alebo použiť v novej otázke.`
      : 'Zdroj môžete vyhľadať alebo použiť v novej otázke.'));
    if (status === 'failed') {
      const retry = el('button', 'request-retry', 'Skúsiť znova'); retry.type = 'button'; retry.disabled = state.requestSubmitting;
      retry.addEventListener('click', () => submitSourceRequest(request.kind, request.reference)); row.append(retry);
    }
    return row;
  });
  $('source-requests').replaceChildren(...(rows.length ? rows : [el('p', 'small muted', 'Zatiaľ nemáte žiadne žiadosti.')]));
}
function scheduleSourceRequestPoll(delay = 2500) {
  clearTimeout(state.requestPollTimer);
  state.requestPollTimer = setTimeout(() => refreshSourceRequests(), delay);
}
async function refreshSourceRequests() {
  if (state.requestLoading) return;
  clearTimeout(state.requestPollTimer);
  state.requestLoading = true;
  $('refresh-source-requests').disabled = true;
  let retryPoll = false;
  try {
    const requests = await api('/api/source-requests');
    if (requests.some(request => request.status === 'completed'
      && !state.sourceRequests.some(previous => previous.id === request.id && previous.status === 'completed'))) state.corpusRefreshNeeded = true;
    if (JSON.stringify(requests) !== JSON.stringify(state.sourceRequests)) {
      state.sourceRequests = requests;
      renderSourceRequests();
    }
    if (state.corpusRefreshNeeded) {
      state.corpus = await api('/api/corpus');
      renderCorpus(state.corpus);
      state.corpusRefreshNeeded = false;
      requestFeedback('Zdroj je dostupný. Knižnica je aktualizovaná.');
    }
  } catch (e) {
    requestFeedback(`Žiadosti sa nepodarilo obnoviť: ${e.message}`, true);
    retryPoll = true;
  } finally {
    state.requestLoading = false;
    $('refresh-source-requests').disabled = false;
    if (!$('login-dialog').open && (retryPoll || state.sourceRequests.some(request => ['queued', 'running'].includes(request.status)))) {
      scheduleSourceRequestPoll(retryPoll ? 10000 : 2500);
    }
  }
}
async function submitSourceRequest(kind, reference) {
  if (state.requestSubmitting || !reference.trim()) return;
  state.requestSubmitting = true;
  $('source-request-submit').disabled = true;
  $('source-request-submit').textContent = 'Odosielam…';
  requestFeedback(''); renderSourceRequests();
  try {
    await api('/api/source-requests', { method: 'POST', body: JSON.stringify({ kind, reference: reference.trim() }) });
    requestFeedback('Žiadosť bola prijatá. Priebeh spracovania nájdete nižšie; môžete pokračovať v rozhovore.');
    if ($('source-request-reference').value.trim() === reference.trim()) $('source-request-reference').value = '';
    await refreshSourceRequests();
    scheduleSourceRequestPoll(1000);
  } catch (e) { requestFeedback(e.message, true); }
  finally {
    state.requestSubmitting = false;
    $('source-request-submit').disabled = false;
    $('source-request-submit').textContent = 'Pridať na pozadí'; renderSourceRequests();
  }
}
async function initialize() {
  const restoring = restorePendingChat();
  try {
    state.corpus = await api('/api/corpus'); renderCorpus(state.corpus);
    try {
      await api('/api/session', { method: 'POST', body: '{}' });
      $('login-dialog').close(); state.recoveryWake?.();
      await refreshHistory(); await refreshSourceRequests();
    }
    catch (failure) { if (failure.status === 401) showLogin(); else throw failure; }
  } catch (e) { error(`Aplikácia nie je dostupná: ${e.message}`); $('mode-label').textContent = 'Nedostupné'; }
  await restoring;
}
function toolLabel(name) {
  return ({ search_norms: 'Hľadanie predpisov', search_judgments: 'Hľadanie judikatúry', get_section: 'Čítanie paragrafu', get_judgment: 'Čítanie judikatúry' })[name] || name;
}
$('chat-form').addEventListener('submit', submit);
$('attach-files').addEventListener('click', () => $('file-input').click());
$('file-input').addEventListener('change', event => uploadFiles(event.target.files));
function isFileDrag(event) { return Array.from(event.dataTransfer?.types || []).includes('Files'); }
$('chat-form').addEventListener('dragover', event => {
  if (!isFileDrag(event)) return;
  event.preventDefault();
  if (!state.busy && !state.attachmentBusy) $('chat-form').classList.add('drag-over');
});
$('chat-form').addEventListener('dragleave', event => {
  if (!event.relatedTarget || !$('chat-form').contains(event.relatedTarget)) $('chat-form').classList.remove('drag-over');
});
$('chat-form').addEventListener('drop', event => {
  if (!isFileDrag(event)) return;
  event.preventDefault(); $('chat-form').classList.remove('drag-over'); uploadFiles(event.dataTransfer.files);
});
// Dropping outside the composer must not navigate away from the conversation.
for (const type of ['dragover', 'drop']) document.addEventListener(type, event => { if (isFileDrag(event)) event.preventDefault(); });
document.addEventListener('visibilitychange', resumeChatConnection);
window.addEventListener('pageshow', event => { if (event.persisted) resumeChatConnection(); });
window.addEventListener('online', resumeChatConnection);
$('new-chat').addEventListener('click', newChat);
$('menu').addEventListener('click', () => $('sidebar').classList.toggle('open'));
$('close-sidebar').addEventListener('click', () => $('sidebar').classList.remove('open'));
$('source-toggle').addEventListener('click', () => { $('sources-panel').classList.toggle('open'); $('sources-panel').classList.toggle('closed'); });
$('question').addEventListener('keydown', event => { if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); $('chat-form').requestSubmit(); } });
document.addEventListener('keydown', event => { if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); newChat(); } });
document.querySelectorAll('[data-question]').forEach(button => button.addEventListener('click', () => { if (!state.busy) { $('question').value = button.dataset.question; $('question').focus(); } }));
document.querySelectorAll('[data-close]').forEach(button => button.addEventListener('click', () => $(button.dataset.close).close()));
$('open-search').addEventListener('click', () => { $('search-dialog').showModal(); $('search-query').focus(); });
$('open-corpus').addEventListener('click', () => { $('corpus-dialog').showModal(); refreshSourceRequests(); });
$('source-request-kind').addEventListener('change', updateSourceRequestKind);
$('source-request-form').addEventListener('submit', event => {
  event.preventDefault(); submitSourceRequest($('source-request-kind').value, $('source-request-reference').value);
});
$('refresh-source-requests').addEventListener('click', () => refreshSourceRequests());
$('login-dialog').addEventListener('cancel', event => event.preventDefault());
$('login-form').addEventListener('submit', async event => {
  event.preventDefault(); $('login-error').textContent = '';
  try { await api('/api/session', { method: 'POST', body: JSON.stringify({ password: $('password').value }) }); $('password').value = ''; $('login-dialog').close(); state.recoveryWake?.(); await refreshHistory(); await refreshSourceRequests(); await restorePendingChat(); }
  catch (e) { $('login-error').textContent = e.message; }
});
$('search-form').addEventListener('submit', async event => {
  event.preventDefault(); $('search-results').replaceChildren(el('p', '', 'Vyhľadávam…'));
  try {
    const params = new URLSearchParams({ q: $('search-query').value });
    const data = await api(`/api/search?${params}`);
    $('search-results').replaceChildren(...data.sources.map(sourceCard));
    if (!data.sources.length) $('search-results').append(el('p', '', data.covered ? 'Nenašli sa zdroje. Skúste právny pojem, číslo paragrafu alebo číslo súdnej veci.' : 'K aktuálnemu dátumu nie je k dispozícii znenie.'));
  } catch (e) { $('search-results').replaceChildren(el('p', 'error', e.message)); }
});
initialize();
