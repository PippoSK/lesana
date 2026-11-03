"""Check provider protocols, repair budgets, and errors without saving invalid answers.

For native retry counts, run inside mock-model with application and fixture URLs:
python -B /tests/openrouter_protocol.py http://agent-test:8080 http://127.0.0.1:9000
"""
import http.cookiejar
import json
import sys
import urllib.request
import uuid
from legal_eval import events

base = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8089"
client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

def request(path, data=None, method=None):
    req = urllib.request.Request(base + path, method=method or ("POST" if data is not None else "GET"),
        headers={"Content-Type": "application/json"}, data=json.dumps(data).encode() if data is not None else None)
    with client.open(req, timeout=90) as response:
        body = response.read().decode()
        if "text/event-stream" in response.headers.get("Content-Type", ""):
            return events(body)
        return json.loads(body) if body else None

request("/api/session", {"password": "test-only-password"})
model = request("/api/corpus")["model"]
assert model in ("gpt-6.1-sol", "openai/gpt-6.1-sol", "qwen/qwen3.8-27b", "deepseek/deepseek-v4.1-flash")
checks = 0
failures = [("RIG_RESEARCH_TRUNCATED RIG_SECTION", "model_output_limit", False),
            ("PROVIDER_AUTH § 181 SSP", "model_configuration", False),
            ("PROVIDER_PAYMENT § 181 SSP", "model_payment", False),
            ("REVIEW_PAYMENT § 181 SSP", "model_payment", True),
            ("SCHEMA_EXTRA_FIELD RIG_SECTION", "model_response", True),
            ("MALFORMED_REVIEW RIG_SECTION", "model_response", True),
            ("DRAFT_OUTPUT_LIMIT § 181 SSP", "model_output_limit", True),
            ("REVIEW_ALWAYS_TRUNCATED § 181 SSP", "model_output_limit", True)]
if model == "gpt-6.1-sol":
    failures += [(f"{trigger} RATECASE_{uuid.uuid4().hex} § 181 SSP", code, True)
                 for trigger, code in (("NATIVE_RATE_LIMIT_ALWAYS", "model_busy"), ("NATIVE_QUOTA_EXHAUSTED", "model_payment"))]
for question, code, retrieved in failures:
    conversation = request("/api/conversations", {})["id"]
    try:
        stream = request(f"/api/conversations/{conversation}/chat", {
            "message": question, "architecture": "agentic"})
        assert not any(kind == "answer" for kind, _ in stream), stream
        assert any(kind == "tool" for kind, _ in stream) == retrieved, stream
        errors = [value for kind, value in stream if kind == "error"]
        assert len(errors) == 1 and errors[0]["code"] == code, errors
        if code == "model_payment":
            assert errors[0]["error"] == "Poskytovateľ modelu odmietol požiadavku pre nedostatok kreditu alebo limit účtu. Skontrolujte kredit a limity u poskytovateľa modelu.", errors
        assert request(f"/api/conversations/{conversation}") == []
        checks += 1
    finally:
        request(f"/api/conversations/{conversation}", method="DELETE")
recoverable = ["REPAIR_DRAFT_FORMAT § 181 SSP", "REPAIR_REVIEW § 181 SSP", "SINGLE_REVIEW_BUDGET § 181 SSP"]
if model in ("deepseek/deepseek-v4.1-flash", "gpt-6.1-sol"):
    recoverable += ["SINGLE_REVIEW_HIGH_BUDGET § 181 SSP", "DRAFT_RETRY_BUDGET § 181 SSP", "RESEARCH_RETRY_BUDGET § 181 SSP"]
if model == "gpt-6.1-sol":
    recoverable += [f"NATIVE_RETRY_{phase} RATECASE_{uuid.uuid4().hex} § 181 SSP" for phase in ("RESEARCH", "ANSWER", "REVIEW")]
for question in recoverable:
    conversation = request("/api/conversations", {})["id"]
    try:
        stream = request(f"/api/conversations/{conversation}/chat", {
            "message": question, "architecture": "agentic"})
        assert not any(kind == "error" for kind, _ in stream), stream
        if question.startswith("RESEARCH_RETRY_BUDGET"):
            assert sum(kind == "tool" for kind, _ in stream) == 1, "Truncated research must not execute its tool calls"
        answer = next(value for kind, value in stream if kind == "answer")
        assert answer["mode"] == "agent" and answer["claims"], answer
        assert [claim["text"] for claim in answer["claims"]] == ["Testovacia odpoveď: overený paragraf o lehote."], answer
        assert len(request(f"/api/conversations/{conversation}")) == 2
        checks += 1
    finally:
        request(f"/api/conversations/{conversation}", method="DELETE")
conversation = request("/api/conversations", {})["id"]
try:
    stream = request(f"/api/conversations/{conversation}/chat", {
        "message": "PARTIAL_REPAIR_TRUNCATED § 181 SSP", "architecture": "agentic"})
    assert not any(kind == "error" for kind, _ in stream), stream
    answer = next(value for kind, value in stream if kind == "answer")
    assert answer["mode"] == "agent_partial", answer
    assert [claim["text"] for claim in answer["claims"]] == ["Testovacia odpoveď: overený paragraf o lehote."], answer
    assert "Nepodložený vedľajší záver" not in answer["answer"], answer
    saved = request(f"/api/conversations/{conversation}")
    assert len(saved) == 2 and saved[-1]["mode"] == "agent_partial", saved
    assert saved[-1]["claims"] == answer["claims"] and saved[-1]["sources"] == answer["sources"], saved
    checks += 1
finally:
    request(f"/api/conversations/{conversation}", method="DELETE")
if model == "gpt-6.1-sol" and len(sys.argv) > 2:
    with urllib.request.urlopen(sys.argv[2] + "/fixture/rate-limits", timeout=10) as response:
        counts = json.load(response)
    expected = {"NATIVE_RETRY_RESEARCH": 2, "NATIVE_RETRY_ANSWER": 2,
                "NATIVE_RETRY_REVIEW": 2, "NATIVE_RATE_LIMIT_ALWAYS": 4,
                "NATIVE_QUOTA_EXHAUSTED": 1}
    for trigger, attempts in expected.items():
        question = next(question for question in recoverable + [item[0] for item in failures] if question.startswith(trigger))
        matching = [item for item in counts if item["trigger"] == trigger and item["case"] in question]
        assert len(matching) == 1 and matching[0]["attempts"] == attempts, (trigger, matching)
print(json.dumps({"status": "passed", "checks": checks, "provider": "openai" if model == "gpt-6.1-sol" else "openrouter", "model": model}))
