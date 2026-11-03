"""Versioned legal retrieval benchmark; legal correctness requires human review.

TEST_PASSWORD=... python3 tests/legal_eval.py --base http://localhost:8089 --output test-results/legal-after.json
Add --chat to collect model answers for review (uses the configured model).
All temporary conversations are deleted. No third-party judge is called.
"""
import argparse
import hashlib
import http.cookiejar
import json
import os
from pathlib import Path
import urllib.parse
import urllib.request


def matches(source, expected, exact=False):
    anchor = source['anchor']
    target = expected['anchor']
    related = anchor == target or (not exact and (anchor.startswith(target + '.') or target.startswith(anchor + '.')))
    return related and expected['act_path'] in source['source_url']


def events(body):
    result = []
    for frame in body.replace('\r\n', '\n').split('\n\n'):
        kind, data = None, []
        for line in frame.splitlines():
            if line.startswith('event:'): kind = line[6:].strip()
            if line.startswith('data:'): data.append(line[5:].strip())
        if kind and data: result.append((kind, json.loads('\n'.join(data))))
    return result


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', default='http://localhost:8089')
    parser.add_argument('--cases', type=Path, default=Path(__file__).with_name('legal_cases.json'))
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--compare', type=Path)
    parser.add_argument('--chat', action='store_true')
    parser.add_argument('--architecture', choices=['pipeline', 'agentic'], help='Chat architecture; otherwise the server default')
    parser.add_argument('--limit', type=int)
    parser.add_argument('--ids', nargs='+', help='Only these case IDs')
    args = parser.parse_args()
    raw = args.cases.read_bytes()
    suite = json.loads(raw)
    client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def request(path, method='GET', data=None):
        req = urllib.request.Request(args.base.rstrip('/') + path, method=method,
            headers={'Content-Type': 'application/json'},
            data=json.dumps(data).encode() if data is not None else None)
        with client.open(req, timeout=630) as response:
            body = response.read().decode()
            if 'text/event-stream' in response.headers.get('Content-Type', ''): return events(body)
            return json.loads(body) if body else None

    request('/api/session', 'POST', {'password': os.environ.get('TEST_PASSWORD', '')})
    corpus = request('/api/corpus')
    if args.chat and not corpus['agent_enabled']:
        raise SystemExit('--chat requires a configured model; research mode is not an answer evaluation')
    actual_hashes = {v['sha256'] for v in corpus['versions']}
    expected_hashes = {v['sha256'] for v in suite['source_snapshots'].values()}
    if actual_hashes != expected_hashes:
        raise SystemExit('Endpoint corpus does not match the benchmark snapshots')
    selected = [c for c in suite['cases'] if not args.ids or c['id'] in args.ids]
    if args.ids and {c['id'] for c in selected} != set(args.ids):
        raise SystemExit('Unknown benchmark case ID')
    if args.limit is not None and args.limit <= 0:
        raise SystemExit('--limit must be positive')
    rows = []
    for case in selected[:args.limit]:
        row = {'id': case['id'], 'category': case['category'], 'question': case['question'],
               'review_status': 'pending', 'legal_correctness': None}
        conversation = None
        try:
            response = request('/api/search?' + urllib.parse.urlencode({'q': case['question'], 'as_of': case['as_of']}))
            sources = response['sources']
            hits = [any(matches(s, e, case['category'] != 'natural') for s in sources) for e in case['expected_sources']]
            extra = case['category'] != 'natural' and any(not any(matches(s, e, True) for e in case['expected_sources']) for s in sources)
            row.update(retrieval_pass=all(hits) and not extra,
                expected_source_recall=sum(hits)/len(hits) if hits else None,
                sources=[{'anchor': s['anchor'], 'source_url': s['source_url']} for s in sources])
            if args.chat:
                conversation = request('/api/conversations', 'POST', {})['id']
                stream = request(f'/api/conversations/{conversation}/chat', 'POST', {'message': case['question'], 'architecture': args.architecture})
                answers = [data for kind, data in stream if kind == 'answer']
                row['answer'] = answers[0] if len(answers) == 1 else None
                row['answer_errors'] = [data for kind, data in stream if kind == 'error']
        except Exception as error:
            row.update(retrieval_pass=False, error=str(error))
        finally:
            if conversation:
                request(f'/api/conversations/{conversation}', 'DELETE')
        rows.append(row)
        print(case['id'], 'PASS' if row['retrieval_pass'] else 'FAIL', flush=True)
    result = {'suite_version': suite['version'], 'suite_sha256': hashlib.sha256(raw).hexdigest(),
        'base': args.base, 'chat': args.chat, 'corpus': corpus, 'cases': rows,
        'summary': {'cases': len(rows), 'retrieval_passed': sum(r['retrieval_pass'] for r in rows),
                    'chat_answers': sum(bool(r.get('answer')) for r in rows) if args.chat else None,
                    'legal_correctness': 'not measured; human review pending'}}
    if args.compare:
        before = json.loads(args.compare.read_text())
        if before['suite_sha256'] != result['suite_sha256']:
            raise SystemExit('Cannot compare different benchmark suites')
        def snapshot_ids(report):
            return sorted((v['number'], v['valid_from'], v['sha256']) for v in report['corpus']['versions'])
        if snapshot_ids(before) != snapshot_ids(result):
            raise SystemExit('Cannot compare different corpus snapshots')
        previous = {r['id']: r for r in before['cases']}
        result['comparison'] = {
            'improved': [r['id'] for r in rows if r['retrieval_pass'] and r['id'] in previous and not previous[r['id']]['retrieval_pass']],
            'regressed': [r['id'] for r in rows if not r['retrieval_pass'] and previous.get(r['id'], {}).get('retrieval_pass')]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(result['summary'], ensure_ascii=False))


if __name__ == '__main__': run()
