"""Hybrid retrieval against an isolated PostgreSQL and deterministic provider.

python3 tests/embeddings.py [http://localhost:8090] [http://localhost:8089]
This checks retrieval plumbing and failure handling, not model quality.
"""
import http.cookiejar
import json
import sys
import time
import urllib.parse
import urllib.request

HYBRID = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8090"
LEXICAL = sys.argv[2] if len(sys.argv) > 2 else "http://localhost:8089"


def client(base):
    browser = urllib.request.build_opener(
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
    req = urllib.request.Request(base + "/api/session",
        data=json.dumps({"password": "test-only-password"}).encode(),
        headers={"Content-Type": "application/json"})
    with browser.open(req, timeout=30) as response:
        assert response.status == 200
    return browser


hybrid, lexical = client(HYBRID), client(LEXICAL)


def get(browser, base, path):
    with browser.open(base + path, timeout=15) as response:
        return json.load(response)


def search(query, date="2026-10-01", semantic=True):
    params = urllib.parse.urlencode({"q": query, "as_of": date})
    browser, base = (hybrid, HYBRID) if semantic else (lexical, LEXICAL)
    return get(browser, base, "/api/search?" + params)["sources"]


deadline = time.monotonic() + 120
while True:
    status = get(hybrid, HYBRID, "/api/corpus")["embeddings"]
    if status["ready"]:
        break
    assert time.monotonic() < deadline, status
    time.sleep(1)
# The bundled treaty adds 96 searchable articles to the previous corpus.
assert status["indexed_nodes"] == status["total_nodes"] == 13337, status
assert not get(lexical, LEXICAL, "/api/corpus")["embeddings"]["enabled"]

# A single synthetic token avoids English words present in the treaty corpus.
query = "QZXVKBEMBEDPROBE"
assert not search(query, semantic=False)
sources = search(query)
assert len(sources) == 1, sources
source = sources[0]
assert source["anchor"] == "paragraf-181.odsek-1", source
assert source["act_title"] == "Správny súdny poriadok"
assert "dvoch mesiacov" in source["text"]
assert source["source_url"].endswith("#paragraf-181.odsek-1")
assert source["retrieval_methods"] == ["embedding"], source
assert search(query + " SSP")
assert source["id"] not in [s["id"] for s in search(query + " CSP")]
assert source["id"] not in [s["id"] for s in search(query + " SP")]
for scope in ("ZoP", "TZ", "TP"):
    assert source["id"] not in [s["id"] for s in search(query + " " + scope)]
assert not search(query, "2026-06-11")
assert search(query, "2026-10-02")[0]["verified_on"] == source["verified_on"]
assert search(query, "2026-06-12")

# The same result found by both branches keeps both methods after fusion.
mixed = search("QZXVKBEMBEDPROBE lehota žaloba SSP")
assert next(s for s in mixed if s["id"] == source["id"])["retrieval_methods"] == ["fulltext", "embedding"], mixed

for trigger in ("EMBEDDING_ERROR", "EMBEDDING_WRONG_DIM", "EMBEDDING_ZERO", "EMBEDDING_MISSING", "EMBEDDING_TIMEOUT"):
    query = trigger + " lehota žaloba"
    started = time.monotonic()
    assert search(query) == search(query, semantic=False), trigger
    assert all(s["retrieval_methods"] == ["fulltext"] for s in search(query, semantic=False)), trigger
    assert time.monotonic() - started < 10, trigger

# Exact citations bypass the provider, even when the query would fail there.
assert search("EMBEDDING_ERROR § 181 SSP") == search("§ 181 SSP", semantic=False)
assert search("§ 181 SSP")[0]["retrieval_methods"] == ["exact"]
assert not search("QZXVKBEMBEDPROBE § 9999 SSP")

# Chat retrieval uses the same hybrid path even without a chat model.
def post(path, data):
    req = urllib.request.Request(HYBRID + path, data=json.dumps(data).encode(),
        headers={"Content-Type": "application/json"})
    return hybrid.open(req, timeout=30)

with post("/api/conversations", {}) as response:
    conversation = json.load(response)["id"]
try:
    with post(f"/api/conversations/{conversation}/chat", {"message": "QZXVKBEMBEDPROBE"}) as response:
        body = response.read().decode()
    events = [json.loads(line[5:]) for line in body.splitlines() if line.startswith("data:")]
    answer = next(event for event in events if "answer" in event)
    assert answer["mode"] == "research"
    assert any(s["id"] == source["id"] for s in answer["sources"]), answer
    assert next(s for s in answer["sources"] if s["id"] == source["id"])["retrieval_methods"] == ["embedding"]
    saved = get(hybrid, HYBRID, f"/api/conversations/{conversation}")
    assert saved[-1]["sources"] == answer["sources"]
finally:
    with hybrid.open(urllib.request.Request(HYBRID + f"/api/conversations/{conversation}", method="DELETE")):
        pass

print("PASS: semantic-only retrieval, citations, act/date filters, lexical fallback, malformed vectors, timeout, exact references and chat")
