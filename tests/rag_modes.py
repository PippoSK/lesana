"""Rig architecture integration checks against the disposable protocol fixture.

TEST_PASSWORD=test-only-password python3 tests/rag_modes.py http://localhost:8089
No real model or legal-quality claims. Every created conversation is removed.
"""
import http.cookiejar
import json
import os
import sys
import urllib.error
import urllib.request

base = sys.argv[1] if len(sys.argv) > 1 else 'http://localhost:8089'
client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
created = []


def request(path, data=None, method=None):
    req = urllib.request.Request(base + path, method=method or ('POST' if data is not None else 'GET'),
        headers={'Content-Type': 'application/json'}, data=json.dumps(data).encode() if data is not None else None)
    with client.open(req, timeout=90) as response:
        body = response.read().decode()
        if 'text/event-stream' in response.headers.get('Content-Type', ''):
            events = []
            for frame in body.replace('\r\n', '\n').split('\n\n'):
                kind, payload = None, None
                for line in frame.splitlines():
                    if line.startswith('event:'): kind = line[6:].strip()
                    if line.startswith('data:'): payload = json.loads(line[5:])
                if kind: events.append((kind, payload))
            return events
        return json.loads(body) if body else None


def chat(question, architecture, conversation=None):
    if conversation is None:
        conversation = request('/api/conversations', {})['id']
        created.append(conversation)
    events = request(f'/api/conversations/{conversation}/chat', {'message': question, 'architecture': architecture})
    answer = next((p for k, p in events if k == 'answer'), None)
    assert answer is not None, events
    assert answer['as_of'] == corpus['default_date'], answer
    assert answer['architecture'] == architecture
    saved = request(f'/api/conversations/{conversation}')[-1]
    assert saved['as_of'] == answer['as_of']
    assert saved['architecture'] == architecture
    assert saved['tool_calls'] == answer['tool_calls']
    assert saved['sources'] == answer['sources'] and saved['claims'] == answer['claims']
    return answer, events, conversation


try:
    request('/api/session', {'password': os.environ.get('TEST_PASSWORD', 'test-only-password')})
    corpus = request('/api/corpus')
    assert corpus['rag']['engine'] == 'rig'
    assert corpus['rag']['architectures'] == ['pipeline', 'agentic']
    checks = 0
    for trigger, tool in [('RIG_NORMS', 'search_norms'), ('RIG_CASE_SEARCH', 'search_judgments'),
                          ('RIG_CASE_EXACT', 'search_judgments'), ('RIG_CASE_FULL', 'get_judgment'),
                          ('RIG_SECTION', 'get_section')]:
        answer, events, _ = chat(trigger, 'agentic')
        assert answer['mode'] == 'agent' and answer['claims'], answer
        assert [c['name'] for c in answer['tool_calls']] == [tool], answer
        assert any(k == 'tool' and p['name'] == tool for k, p in events)
        if 'CASE' in trigger: assert all(s['judgment'] for s in answer['sources'])
        checks += 1
    answer, _, conversation = chat('RIG_SECTION § 181 SSP', 'pipeline')
    assert answer['claims'] and not answer['tool_calls'], answer
    answer, _, _ = chat('RIG_SECTION § 181 SSP', 'agentic', conversation=conversation)
    assert answer['claims'] and len(answer['tool_calls']) == 1, answer
    answer, _, _ = chat('RIG_SECTION § 181 SSP', 'pipeline', conversation=conversation)
    assert answer['claims'] and not answer['tool_calls'], answer
    checks += 3
    for trigger in ['RIG_NO_TOOLS § 181 SSP', 'RIG_INVALID']:
        conversation = request('/api/conversations', {})['id']
        created.append(conversation)
        events = request(f'/api/conversations/{conversation}/chat',
                         {'message': trigger, 'architecture': 'agentic'})
        assert not any(kind == 'answer' for kind, _ in events), events
        errors = [payload for kind, payload in events if kind == 'error']
        assert len(errors) == 1 and errors[0]['code'] == 'model_response', events
        assert request(f'/api/conversations/{conversation}') == []
        checks += 1
    for trigger in ['RIG_UNKNOWN_CASE']:
        answer, _, _ = chat(trigger, 'agentic')
        assert answer['mode'] == 'no_sources' and not answer['sources'] and not answer['claims'], answer
        checks += 1
    answer, _, conversation = chat('RIG_HISTORY_SEED RIG_SECTION § 181 SSP', 'agentic')
    assert 'Bol vám dokument doručený 2. októbra?' in answer['answer'], answer
    for reply in ['Áno.', 'Opravujem dátum: bolo to 3. októbra.']:
        previous = request(f'/api/conversations/{conversation}')
        expected = [{'role': message['role'], 'content': message['content']} for message in previous]
        question = 'RIG_RETRY_LOOKUP ' + json.dumps(
            {'question': reply, 'history': expected}, ensure_ascii=False)
        answer, events, _ = chat(question, 'agentic', conversation=conversation)
        assert answer['mode'] == 'agent' and answer['claims'], answer
        assert [call['name'] for call in answer['tool_calls']] == ['get_section'], answer
        assert answer['tool_calls'][0]['outcome'] == 'ok', answer
        assert not any(kind == 'error' for kind, _ in events), events
        saved = request(f'/api/conversations/{conversation}')
        assert len(saved) == len(previous) + 2
        assert [{'role': message['role'], 'content': message['content']}
                for message in saved[:-2]] == expected
        assert saved[-2]['content'] == question
        assert saved[-1]['content'] == answer['answer']
        checks += 1
    answer, events, _ = chat('RIG_LOOP', 'agentic')
    assert len(answer['tool_calls']) == corpus['rag']['max_turns'], answer
    assert any(k == 'status' and 'limit rešerše' in p['text'] for k, p in events), events
    checks += 1
    conversation = request('/api/conversations', {})['id']
    created.append(conversation)
    try:
        request(f'/api/conversations/{conversation}/chat', {'message': 'test', 'architecture': 'unknown'})
        raise AssertionError('Unknown architecture accepted')
    except urllib.error.HTTPError as error:
        assert error.code == 422
    assert not request(f'/api/conversations/{conversation}')
    checks += 1
    print(json.dumps({'status':'passed', 'checks':checks, 'engine':'rig'}, indent=2))
finally:
    for conversation in created:
        request(f'/api/conversations/{conversation}', method='DELETE')
