"""Exercise one real-model chat without retaining a test conversation."""
import argparse
import http.cookiejar
import json
import os
from pathlib import Path
import shlex
import time
import urllib.request
from legal_eval import events

parser = argparse.ArgumentParser()
parser.add_argument('--base', default='http://localhost:8088')
question_input = parser.add_mutually_exclusive_group(required=True)
question_input.add_argument('--question')
question_input.add_argument('--question-file', type=Path)
parser.add_argument('--follow-up', action='append', default=[])
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--env-file', type=Path)
parser.add_argument('--timeout', type=float, default=630, help='HTTP timeout in seconds; use 1830 for native Sol or DeepSeek max')
parser.add_argument('--architecture', choices=['pipeline', 'agentic'], help='Override the server default')
args = parser.parse_args()
if args.question_file:
    args.question = args.question_file.read_text().strip()
password = os.environ.get('TEST_PASSWORD', '')
if args.env_file:
    for line in args.env_file.read_text().splitlines():
        if line.startswith('APP_PASSWORD='):
            value = shlex.split(line.split('=', 1)[1], comments=True)
            password = value[0] if value else ''
client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
def request(path, method='GET', data=None):
    req = urllib.request.Request(args.base + path, method=method, headers={'Content-Type':'application/json'}, data=json.dumps(data).encode() if data is not None else None)
    with client.open(req, timeout=args.timeout) as response:
        body = response.read().decode()
        return events(body) if 'text/event-stream' in response.headers.get('Content-Type','') else (json.loads(body) if body else None)
request('/api/session', 'POST', {'password':password})
corpus = request('/api/corpus')
conversation = request('/api/conversations', 'POST', {})['id']
start = time.monotonic()
try:
    stream = request(f'/api/conversations/{conversation}/chat', 'POST', {'message':args.question, 'architecture':args.architecture})
    answer = next((value for kind,value in stream if kind == 'answer'), None)
    errors = [value for kind,value in stream if kind == 'error']
    history = request(f'/api/conversations/{conversation}')
    result = {'question':args.question,'model':corpus['model'],'as_of':answer['as_of'] if answer else None,'elapsed_seconds':round(time.monotonic()-start,2),'answer':answer,'errors':errors,'progress':[{'event':kind,'data':value} for kind,value in stream if kind not in ('answer','error')],'persisted_messages':len(history)}
    if answer:
        assert len(history)==2 and history[1]['claims']==answer['claims']
    if args.follow_up and answer:
        result['follow_ups'] = []
        for index, question in enumerate(args.follow_up):
            follow_start = time.monotonic()
            stream = request(f'/api/conversations/{conversation}/chat', 'POST', {'message':question, 'architecture':args.architecture})
            follow_answer = next((value for kind,value in stream if kind == 'answer'), None)
            follow_errors = [value for kind,value in stream if kind == 'error']
            history = request(f'/api/conversations/{conversation}')
            if follow_answer:
                assert len(history)==2*(index+2) and history[-1]['claims']==follow_answer['claims']
            result['follow_ups'].append({'question':question,'answer':follow_answer,'errors':follow_errors,'elapsed_seconds':round(time.monotonic()-follow_start,2),'persisted_messages':len(history)})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps({'mode':answer['mode'] if answer else None,'claims':len(answer['claims']) if answer else 0,'errors':errors,'elapsed_seconds':result['elapsed_seconds']},ensure_ascii=False))
finally:
    request(f'/api/conversations/{conversation}','DELETE')
