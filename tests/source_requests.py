"""HTTP checks for background imports; bundled sources need no external network.

TEST_PASSWORD=test-only-password python3 tests/source_requests.py http://localhost:8091
"""
import datetime
import http.cookiejar
import json
import os
from pathlib import Path
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parent.parent
BASE = (sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8088").rstrip("/")
PASSWORD = os.environ.get("TEST_PASSWORD", "")
ENDPOINT = "/api/source-requests"

def client():
    return urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

def request(browser, path, method="GET", data=None, expected=200, headers=None):
    req = urllib.request.Request(BASE + path, method=method,
        headers={"Content-Type": "application/json", **(headers or {})},
        data=json.dumps(data).encode() if data is not None else None)
    try:
        with browser.open(req, timeout=30) as response:
            status, body = response.status, response.read().decode()
    except urllib.error.HTTPError as error:
        status, body = error.code, error.read().decode()
    assert status == expected, (method, path, status, expected, body)
    return json.loads(body) if body else None

def validate(record):
    assert {"id", "kind", "reference", "status", "message", "act_id", "title", "node_count", "created_at", "updated_at"} <= record.keys(), record
    uuid.UUID(record["id"])
    assert record["kind"] in {"legislation", "judgment", "eu_regulation", "supreme_court_judgment", "supreme_court_collection", "czech_supreme_court_judgment"}, record
    assert record["status"] in {"queued", "running", "completed", "failed"}, record
    assert isinstance(record["reference"], str) and record["reference"].strip(), record
    assert record["message"] is None or isinstance(record["message"], str), record
    for field in ("created_at", "updated_at"):
        datetime.datetime.fromisoformat(record[field].replace("Z", "+00:00"))

def submit(browser, kind, reference, expected=202, headers=None):
    result = request(browser, ENDPOINT, "POST", {"kind": kind, "reference": reference}, expected, headers)
    if expected == 202:
        validate(result)
    return result

def listed(browser):
    records = request(browser, ENDPOINT)
    assert isinstance(records, list), records
    for record in records:
        validate(record)
    result = {record["id"]: record for record in records}
    assert len(result) == len(records), records
    return result

def completed(browser, identifier):
    deadline = time.monotonic() + 45
    while True:
        records = listed(browser)
        assert identifier in records, (identifier, records)
        record = records[identifier]
        assert record["status"] != "failed", record
        if record["status"] == "completed":
            assert record["act_id"] and record["title"] and record["node_count"] > 0, record
            return record
        assert time.monotonic() < deadline, ("Background request did not finish", record)
        time.sleep(0.2)

def search(browser, query, date):
    return request(browser, "/api/search?" + urllib.parse.urlencode({"q": query, "as_of": date}))["sources"]

def main():
    checks = []
    first, second, outsider = client(), client(), client()
    ssp = json.loads((ROOT / "data/manifest.json").read_text())
    gruber = json.loads((ROOT / "data/gruber-20050120.json").read_text())
    request(first, ENDPOINT, expected=401)
    submit(first, "legislation", ssp["source_url"], expected=401)
    for cookie in ("lexa_session=invalid", f"lexa_session={uuid.uuid4()}"):
        request(first, ENDPOINT, expected=401, headers={"Cookie": cookie})
        submit(first, "legislation", ssp["source_url"], expected=401, headers={"Cookie": cookie})
    for browser in (first, second, outsider):
        request(browser, "/api/session", "POST", {"password": PASSWORD})
        assert not listed(browser), "New sessions must not inherit other request histories"
    submit(first, "legislation", ssp["source_url"], expected=403, headers={"Origin": "https://untrusted.example"})
    assert not listed(first), "Rejected writes must not create requests"
    checks.append("authentication and cross-origin protection")
    invalid = [
        ("unknown", "162/2015"), ("legislation", ""), ("judgment", "   "),
        ("legislation", "predpis ktorý nepoznám"), ("judgment", "162/2015"),
        ("legislation", "62001CJ0464"), ("legislation", "file:///etc/passwd"),
        ("legislation", "http://localhost:8080/health"), ("legislation", "https://localhost/health"),
        ("legislation", "https://127.0.0.1/"), ("legislation", "https://example.com/legal.html"),
        ("legislation", ssp["source_url"].replace("https:", "http:")),
        ("legislation", ssp["source_url"].replace("static.slov-lex.sk/", "static.slov-lex.sk:444/")),
        ("legislation", ssp["source_url"].replace("https://", "https://user:secret@")),
        ("legislation", ssp["source_url"].replace("static.slov-lex.sk", "static.slov-lex.sk.evil.example")),
        ("judgment", gruber["source_url"].replace("eur-lex.europa.eu", "eur-lex.europa.eu.evil.example")),
        ("judgment", gruber["source_url"].replace("/HTML/", "/PDF/")),
        ("judgment", "62001CJ0464" + "x" * 3000),
        ("eu_regulation", ""), ("eu_regulation", "62001CJ0464"),
        ("eu_regulation", "32012L1215"), ("eu_regulation", "02012R1215-20150226"),
        ("eu_regulation", "https://eur-lex.europa.eu.evil.example/legal-content/SK/TXT/HTML/?uri=CELEX:32012R1215"),
        ("eu_regulation", "https://eur-lex.europa.eu/legal-content/SK/TXT/PDF/?uri=CELEX:32012R1215"),
    ]
    for kind, reference in invalid:
        assert submit(first, kind, reference, expected=400).get("error"), (kind, reference)
    assert not listed(first), "Invalid input must not enter the queue"
    checks.append("invalid inputs and source URL restrictions")
    statute = submit(first, "legislation", ssp["source_url"])
    assert statute["kind"] == "legislation", statute
    assert submit(first, "legislation", "  " + ssp["source_url"] + "  ")["id"] == statute["id"]
    assert set(listed(first)) == {statute["id"]}
    assert not listed(second) and not listed(outsider)
    judgment = submit(second, "judgment", gruber["judgment"]["celex"])
    assert judgment["kind"] == "judgment" and judgment["id"] != statute["id"], judgment
    assert set(listed(first)) == {statute["id"]}
    assert set(listed(second)) == {judgment["id"]}
    assert submit(first, "judgment", gruber["source_url"])["id"] == judgment["id"]
    assert submit(second, "legislation", ssp["source_url"])["id"] == statute["id"]
    expected_ids = {statute["id"], judgment["id"]}
    assert set(listed(first)) == set(listed(second)) == expected_ids
    assert not listed(outsider)
    checks.append("canonical duplicates and session-scoped subscriptions")
    versions = {v["number"]: v for v in request(first, "/api/corpus")["versions"]}
    for record, manifest in ((statute, ssp), (judgment, gruber)):
        result = completed(first, record["id"])
        assert result["act_id"] == manifest["act_id"] and result["title"] == manifest["title"], result
        assert result["node_count"] == versions[manifest["number"]]["nodes"], result
        assert completed(second, record["id"])["act_id"] == result["act_id"]
        repeated = submit(first, record["kind"], manifest["source_url"])
        assert repeated["id"] == result["id"] and repeated["status"] == "completed", repeated
    sources = search(first, "§ 181 SSP", ssp["verified_on"])
    assert any(s["anchor"] == "paragraf-181" and s["source_url"].startswith(ssp["source_url"]) for s in sources), sources
    sources = search(second, gruber["judgment"]["celex"], gruber["verified_on"])
    assert len(sources) == 1 and sources[0]["kind"] == "judgment", sources
    assert sources[0]["source_url"] == gruber["source_url"], sources
    checks.append("background completion, metadata and subsequent retrieval")
    print(json.dumps({"status": "passed", "checks": checks}, ensure_ascii=False, indent=2))

if __name__ == "__main__":
    main()
