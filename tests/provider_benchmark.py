#!/usr/bin/env python3
"""Opt-in billed OpenRouter probes; no production changes or tool execution.

Replays Lesana's initial research prompt and a three-claim evidence review.
The third claim is deliberately false. These are individual API calls, not an
end-to-end benchmark. The socket timeout is not a total wall-clock limit;
completion times are separately checked against Lesana's call deadline.
Response text/reasoning and credentials are never saved.
"""
import argparse
import concurrent.futures
import copy
import hashlib
import json
from pathlib import Path
import re
import shlex
import ssl
import time
from datetime import datetime, timezone
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
MODEL = 'deepseek/deepseek-v4.1-flash'
URL = 'https://openrouter.ai/api/v1/chat/completions'
# python.org macOS installs may lack their default CA symlink. Use the system
# CA bundle in that case, retaining certificate and hostname verification.
CA_BUNDLE = '/etc/ssl/cert.pem' if not ssl.get_default_verify_paths().cafile and Path('/etc/ssl/cert.pem').is_file() else None
TLS_CONTEXT = ssl.create_default_context(cafile=CA_BUNDLE)
STRING = r'"(?:[^"\\]|\\.)*"'


def payloads():
    saved = json.loads((ROOT / 'test-results/deepseek-v4.1-flash-premlcanie.json').read_text())
    agent = (ROOT / 'src/application/usecases/agent.rs').read_text()
    research = (ROOT / 'src/adapters/spi/http/research_tools.rs').read_text()
    system = re.search(r'let system = format!\(\s*r#"(.*?)"#', agent, re.S).group(1)
    system = system.replace('{date}', saved['as_of'])
    preamble = json.loads(re.search(r'\.preamble\(format!\((' + STRING + r')\)', research).group(1))
    system = preamble.replace('{system}', system)
    date = {
        'type': ['string', 'null'], 'format': 'date',
        'description': "Optional research date in YYYY-MM-DD, no later than the conversation's selected date. Use the relevant event date from the question/history for historical legislation. Omit or use null for the selected date. Each call filters sources and linked context to this date; it does not change the conversation date."
    }
    functions = {}
    for name, description in re.findall(r'query_tool!\(\s*\w+,\s*(' + STRING + r'),\s*(' + STRING + r')\s*\);', research):
        name, description = json.loads(name), json.loads(description)
        functions[name] = {'type': 'function', 'function': {
            'name': name, 'description': description,
            'parameters': {'type': 'object', 'properties': {'query': {'type': 'string'}, 'as_of': date}, 'required': ['query'], 'additionalProperties': False}
        }}
    section = research[research.index('impl Tool for GetSection'):]
    description = json.loads(re.search(r'fn description.*?(' + STRING + r')\.into\(\)', section, re.S).group(1))
    functions['get_section'] = {'type': 'function', 'function': {
        'name': 'get_section', 'description': description,
        'parameters': {'type': 'object', 'properties': {'section': {'type': 'string'}, 'act': {'type': 'string'}, 'as_of': date}, 'required': ['section', 'act'], 'additionalProperties': False}
    }}
    tools = [functions[name] for name in ['search_norms', 'search_judgments', 'get_section', 'get_judgment']]
    sources = {s['id']: {k: v for k, v in s.items() if k != 'retrieval_methods'} for s in saved['answer']['sources']}
    claims = [{'index': i, 'text': c['text'], 'evidence': [{'quote': e['quote'], 'source': sources[e['source_id']]} for e in c['evidence']]} for i, c in enumerate(saved['answer']['claims'][:2])]
    claims.append({'index': 2, 'text': 'Citované ustanovenie § 397 Obchodného zákonníka stanovuje všeobecnú premlčaciu dobu 84 rokov.', 'evidence': copy.deepcopy(claims[0]['evidence'])})
    systems = [json.loads(s) for s in re.findall(r'ModelMessage::system\((' + STRING + r')\)', agent)]
    review_system = next(s for s in systems if s.startswith('Si kontrolór'))
    review_group = next(s for s in systems if s.startswith('Kontroluješ iba'))
    schema = {'type': 'object', 'properties': {
        'claims': {'type': 'array', 'items': {'type': 'object', 'properties': {'index': {'type': 'integer'}, 'supported': {'type': 'boolean'}, 'reason': {'type': 'string'}}, 'required': ['index', 'supported', 'reason'], 'additionalProperties': False}},
        'questions_safe': {'type': 'boolean'}
    }, 'required': ['claims', 'questions_safe'], 'additionalProperties': False}
    return {
        'research': {'model': MODEL, 'messages': [{'role': 'system', 'content': system}, {'role': 'user', 'content': saved['question']}], 'tools': tools, 'max_tokens': 16384},
        'review': {'model': MODEL, 'messages': [{'role': 'system', 'content': review_system}, {'role': 'user', 'content': json.dumps({'question': saved['question'], 'conversation_history': [], 'as_of': saved['as_of'], 'claims': claims, 'questions': []}, ensure_ascii=False)}, {'role': 'system', 'content': review_group}], 'max_completion_tokens': 6000, 'response_format': {'type': 'json_schema', 'json_schema': {'name': 'lesana_evidence_review', 'strict': True, 'schema': schema}}}
    }


def validate(workload, payload, choice):
    message = choice['message']
    if workload == 'research':
        assert choice['finish_reason'] == 'tool_calls', 'expected_tool_calls'
        calls = message.get('tool_calls', [])
        assert calls, 'missing_tools'
        schemas = {tool['function']['name']: tool['function']['parameters'] for tool in payload['tools']}
        for call in calls:
            function = call['function']
            schema = schemas[function['name']]
            args = json.loads(function['arguments'])
            assert isinstance(args, dict), 'tool_arguments_object'
            assert set(schema['required']) <= set(args) <= set(schema['properties']), 'tool_argument_keys'
            for k, v in args.items():
                if k == 'as_of':
                    assert v is None or (isinstance(v, str) and re.fullmatch(r'\d{4}-\d{2}-\d{2}', v)), 'tool_date'
                else:
                    assert isinstance(v, str) and v.strip(), 'tool_string'
        return {'tool_names': [c['function']['name'] for c in calls]}
    assert choice['finish_reason'] == 'stop', 'expected_stop'
    value = json.loads(message['content'])
    assert isinstance(value, dict) and set(value) == {'claims', 'questions_safe'}, 'review_root'
    assert type(value['questions_safe']) is bool, 'questions_safe_type'
    rows = value['claims']
    assert isinstance(rows, list) and len(rows) == 3, 'review_count'
    for row in rows:
        assert isinstance(row, dict) and set(row) == {'index', 'supported', 'reason'}, 'review_keys'
        assert type(row['index']) is int and type(row['supported']) is bool and isinstance(row['reason'], str), 'review_types'
        assert (row['reason'] == '' if row['supported'] else 0 < len(row['reason']) <= 250), 'review_reason'
    assert sorted(r['index'] for r in rows) == [0, 1, 2], 'review_indexes'
    assert next(r for r in rows if r['index'] == 2)['supported'] is False, 'false_control_accepted'
    return {'review_verdicts': [{'index': r['index'], 'supported': r['supported']} for r in rows], 'false_control_rejected': True}


def probe(key, provider, workload, base, effort, repeat):
    payload = copy.deepcopy(base)
    payload['reasoning_effort'] = effort
    payload['provider'] = {'only': [provider], 'require_parameters': True, 'allow_fallbacks': False}
    encoded = json.dumps(payload, ensure_ascii=False).encode()
    result = {'provider_requested': provider, 'workload': workload, 'reasoning_effort': effort, 'repeat': repeat, 'started_at': datetime.now(timezone.utc).isoformat(), 'payload_sha256': hashlib.sha256(encoded).hexdigest(), 'valid': False}
    request = urllib.request.Request(URL, data=encoded, headers={'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json', 'X-Title': 'Lesana provider comparison'})
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=125, context=TLS_CONTEXT) as response:
            body = json.load(response)
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        result['provider_actual'] = body.get('provider')
        result['response_id'] = body.get('id')
        usage = body.get('usage', {})
        result['usage'] = usage
        choice = body['choices'][0]
        result['finish_reason'] = choice.get('finish_reason')
        result['content_chars'] = len(choice.get('message', {}).get('content') or '')
        result.update(validate(workload, payload, choice))
        result['valid'] = True
    except urllib.error.HTTPError as error:
        result['http_status'] = error.code
        result['error'] = 'HTTPError'
    except Exception as error:
        result['error'] = type(error).__name__
        if isinstance(error, AssertionError):
            result['validation_failure'] = str(error)
    result.setdefault('elapsed_seconds', round(time.monotonic() - started, 3))
    result['lesana_call_deadline_seconds'] = 125 if workload == 'research' else 120
    result['within_lesana_call_deadline'] = result['elapsed_seconds'] <= result['lesana_call_deadline_seconds']
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--providers', nargs='+', required=True)
    parser.add_argument('--effort', default='max', choices=['high', 'max'])
    parser.add_argument('--repeats', type=int, default=1)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    assert 1 <= args.repeats <= 3
    key = None
    for line in (ROOT / '.env').read_text().splitlines():
        if line.startswith('OPENROUTER_API_KEY='):
            key = shlex.split(line.partition('=')[2])[0]
            break
    if not key:
        raise SystemExit('OPENROUTER_API_KEY not configured')
    cases = payloads()
    output = ROOT / args.output
    output.parent.mkdir(parents=True, exist_ok=True)
    report = {'model': MODEL, 'started_at': datetime.now(timezone.utc).isoformat(), 'scope': 'Individual initial research and strict three-claim review calls, not full Lesana answers', 'reasoning_effort': args.effort, 'max_concurrency': 4, 'socket_timeout_seconds': 125, 'results': []}
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        jobs = [pool.submit(probe, key, provider, workload, payload, args.effort, repeat) for repeat in range(1, args.repeats + 1) for workload, payload in cases.items() for provider in args.providers]
        for job in concurrent.futures.as_completed(jobs):
            result = job.result()
            report['results'].append(result)
            output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
            print(json.dumps(result, ensure_ascii=False), flush=True)
    report['finished_at'] = datetime.now(timezone.utc).isoformat()
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__':
    main()
