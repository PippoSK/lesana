"""Backfill lifecycle checks, exclusively in the disposable Compose test DB.

Run after tests/embeddings.py, with the same Compose test project running.
"""
import json
import http.cookiejar
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
COMPOSE = ["docker", "compose", "-p", os.environ.get("TEST_COMPOSE_PROJECT", "lesana-embedding-check"), "-f", "compose.test.yaml", "--profile", "test"]


def compose(*args):
    return subprocess.run(COMPOSE + list(args), cwd=ROOT, capture_output=True, text=True)


def sql(query):
    result = compose("exec", "-T", "test-db", "psql", "-U", "lexa", "-d", "lexa_test",
                     "-At", "-v", "ON_ERROR_STOP=1", "-c", query)
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def embed(expected):
    result = compose("run", "--rm", "--no-deps", "embedding-test", "embed")
    assert result.returncode == 0, result.stderr
    assert f"Indexed {expected} nodes" in result.stdout, result.stdout[-1000:]


def fingerprint():
    return sql("SELECT md5(string_agg(node_id::text||content_hash||indexed_at::text, ',' ORDER BY node_id)) FROM legal_embedding_documents")


assert sql("SELECT current_database()") == "lexa_test"
assert compose("stop", "embedding-test").returncode == 0
originals = json.loads(sql("SELECT json_agg(t) FROM (SELECT id,context_text FROM legal_nodes WHERE searchable ORDER BY id LIMIT 2) t"))
try:
    before = fingerprint()
    embed(0)
    assert fingerprint() == before, "Completed nodes were embedded again"
    first, second = (item["id"] for item in originals)
    sql(f"UPDATE legal_nodes SET context_text=context_text||E'\nEMBEDDING_ERROR' WHERE id={first}")
    sql(f"UPDATE legal_nodes SET context_text=context_text||E'\nChanged test context' WHERE id={second}")
    result = compose("run", "--rm", "--no-deps", "embedding-test", "embed")
    assert result.returncode != 0 and "HTTP 503" in result.stderr, result.stderr
    assert fingerprint() == before, "A failed batch partially committed"
    # Restore the first document; only the second now needs to be re-embedded.
    content = originals[0]["context_text"].replace("'", "''")
    sql(f"UPDATE legal_nodes SET context_text='{content}' WHERE id={first}")
    embed(1)
    assert fingerprint() != before, "Changed context was not reindexed"
    embed(0)

    # A second model with a different dimension must never enter current search.
    sql("""DELETE FROM legal_embedding_documents WHERE model_key='foreign-fixture';
        DELETE FROM embedding_models WHERE model_key='foreign-fixture';
        INSERT INTO embedding_models VALUES ('foreign-fixture','other-model',2);
        INSERT INTO legal_embedding_documents(model_key,node_id,content_hash)
        SELECT 'foreign-fixture',node_id,content_hash FROM legal_embedding_inputs LIMIT 1;
        INSERT INTO legal_node_embeddings(model_key,node_id,chunk_index,dimensions,embedding)
        SELECT model_key,node_id,0,2,'[1,0]'::vector FROM legal_embedding_documents WHERE model_key='foreign-fixture';""")
finally:
    for item in originals:
        content = item["context_text"].replace("'", "''")
        sql(f"UPDATE legal_nodes SET context_text='{content}' WHERE id={item['id']}")
    assert compose("start", "embedding-test").returncode == 0

browser = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
deadline = time.monotonic() + 30
while True:
    try:
        with browser.open("http://localhost:8090/api/corpus", timeout=5) as response:
            if json.load(response)["embeddings"]["ready"]:
                break
    except (urllib.error.URLError, ConnectionError):
        pass
    assert time.monotonic() < deadline, "Test app did not become ready"
    time.sleep(0.5)
login = urllib.request.Request("http://localhost:8090/api/session", data=b'{"password":"test-only-password"}',
                               headers={"Content-Type": "application/json"})
with browser.open(login) as response:
    assert response.status == 200
with browser.open("http://localhost:8090/api/search?q=QZXVKBEMBEDPROBE") as response:
    sources = json.load(response)["sources"]
    assert len(sources) == 1 and sources[0]["anchor"] == "paragraf-181.odsek-1", sources

print("PASS: idempotent backfill, changed-context indexing, failed-batch rollback and separate model dimensions")
