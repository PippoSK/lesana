"""Live EU regulation import checks; run against a disposable test deployment.

Uses the official Cellar API; never fabricates legal text. The original Brussels Ia
regulation is imported into the target shared corpus, so use the isolated test app.
TEST_PASSWORD=test-only-password python3 tests/eu_regulations.py http://localhost:8089
"""
import datetime
import json
import time

import source_requests as api


def main():
    first, second = api.client(), api.client()
    for browser in (first, second):
        api.request(browser, "/api/session", "POST", {"password": api.PASSWORD})
    celex = "32012R1215"
    url = f"https://eur-lex.europa.eu/legal-content/SK/TXT/HTML/?uri=CELEX:{celex}"
    job = api.submit(first, "eu_regulation", celex)
    assert job["kind"] == "eu_regulation", job
    assert api.submit(first, "eu_regulation", f"celex:{celex.lower()}")["id"] == job["id"]
    assert not api.listed(second), "A new session must not inherit another session's requests"
    assert api.submit(second, "eu_regulation", url)["id"] == job["id"]
    deadline = time.monotonic() + 210
    while True:
        result = api.listed(first)[job["id"]]
        assert result["status"] != "failed", result
        if result["status"] == "completed":
            break
        assert time.monotonic() < deadline, result
        time.sleep(0.5)
    assert result["act_id"] == f"EU/REG/{celex}" and result["node_count"] >= 82, result
    assert api.submit(first, "eu_regulation", url)["status"] == "completed"
    corpus = api.request(first, "/api/corpus")
    versions = [v for v in corpus["versions"] if (v.get("regulation") or {}).get("celex") == celex]
    assert len(versions) == 1, versions
    version = versions[0]
    assert version["articles"] == 81, version
    assert version["regulation"]["language"] == "sk", version
    assert version["regulation"]["published_on"] == version["valid_from"] == "2012-12-20", version
    assert version["source_url"] == url, version
    date = version["verified_on"]
    for query in (f"čl. 1 CELEX:{celex}", "čl. 1 nariadenia 1215/2012"):
        sources = api.search(first, query, date)
        assert len(sources) == 1 and sources[0]["anchor"] == "predpis.text2.clanok-1", (query, sources)
        source = sources[0]
        assert source["regulation"]["celex"] == celex and source["judgment"] is None and source["treaty"] is None, source
        assert "občianskych a obchodných veciach" in source["text"], source
        assert "nariadenia" in source["citation"] and "zákona č." not in source["citation"], source
        assert source["source_url"] == url and "#" not in source["source_url"], source
        stored = api.request(first, f"/api/nodes/{source['id']}")
        assert stored["regulation"] == source["regulation"], stored
    entire = api.search(first, celex, date)
    assert len(entire) == 1 and entire[0]["kind"] == "eu_regulation", entire
    assert api.search(first, f"čl. 1 CELEX:{celex}", "2012-12-20")
    assert not api.search(first, f"čl. 1 CELEX:{celex}", "2012-12-19")
    after_verification = (datetime.date.fromisoformat(date) + datetime.timedelta(days=1)).isoformat()
    assert not api.search(first, f"čl. 1 CELEX:{celex}", after_verification)
    assert not api.search(first, f"čl. 1 ods. 999 CELEX:{celex}", date)
    assert not api.search(first, "čl. 1 nariadenia 1216/2012", date)
    print(json.dumps({"status": "passed", "celex": celex, "articles": version["articles"],
        "checks": ["background import", "CELEX/URL deduplication", "session isolation", "official metadata", "exact article retrieval", "source detail", "publication and verification boundaries", "no invented paragraphs"]}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
