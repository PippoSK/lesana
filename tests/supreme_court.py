"""Live NSSR PDF import checks against an isolated test deployment.

TEST_PASSWORD=test-only-password python3 -B tests/supreme_court.py http://localhost:8091
"""
import datetime
import json
import time

import source_requests as api


def main():
    first, second = api.client(), api.client()
    for browser in (first, second):
        api.request(browser, "/api/session", "POST", {"password": api.PASSWORD})
    kind = "supreme_court_judgment"
    url = "https://www.nsud.sk/data/att/96d/805912.7b72da.pdf"
    ecli = "ECLI:SK:NSSR:2024:0021203612.1"
    for invalid in ("4Ndc/31/2024", ecli, url.replace("www.nsud.sk", "www.nsud.sk.evil.example"),
                    url.replace("https:", "http:"), url + "?redirect=http://localhost"):
        api.submit(first, kind, invalid, expected=400)
    assert not api.listed(first)
    job = api.submit(first, kind, url)
    assert api.submit(first, kind, url + "#page=2")["id"] == job["id"]
    assert not api.listed(second)
    assert api.submit(second, kind, url.replace("www.nsud.sk", "nsud.sk"))["id"] == job["id"]
    deadline = time.monotonic() + 100
    while True:
        result = api.listed(first)[job["id"]]
        assert result["status"] != "failed", result
        if result["status"] == "completed":
            break
        assert time.monotonic() < deadline, result
        time.sleep(0.3)
    assert result["node_count"] == 6, result
    assert api.submit(first, kind, url)["status"] == "completed"
    corpus = api.request(first, "/api/corpus")
    versions = [v for v in corpus["versions"] if (v.get("judgment") or {}).get("ecli") == ecli]
    assert len(versions) == 1, versions
    version = versions[0]
    assert version["judgment"]["expected_pages"] == 4, version
    assert version["judgment"]["decided_on"] == version["valid_from"] == "2024-11-28", version
    assert version["source_url"] == version["pdf_url"] == url, version
    date = version["verified_on"]
    for query in ("4Ndc/31/2024 strana 2", "4 Ndc / 31 / 2024 strana 2", ecli + " strana 2"):
        found = api.search(first, query, date)
        assert len(found) == 1 and found[0]["anchor"] == "judgment-page-2", (query, found)
        source = found[0]
        assert source["source_url"] == url + "#page=2", source
        assert "strana 2 PDF" in source["citation"], source
        stored = api.request(first, f"/api/nodes/{source['id']}")
        assert stored["judgment"] == source["judgment"], stored
    whole = api.search(first, ecli, date)
    assert len(whole) == 1 and whole[0]["kind"] == "judgment", whole
    assert "Poučenie" in whole[0]["text"] and "1.1" in whole[0]["text"], whole
    operative = api.search(first, "4Ndc/31/2024 výrok", date)
    assert len(operative) == 1 and operative[0]["kind"] == "judgment_operative", operative
    assert "Na prejednanie sporu je príslušný Krajský súd v Prešove." in operative[0]["text"], operative
    assert "Odôvodnenie" not in operative[0]["text"]
    for query in (ecli + "0", "4Ndc/32/2024", "4Ndc/31/2024 bod 2", "4Ndc/31/2024 strana 99"):
        assert not api.search(first, query, date), query
    assert not api.search(first, ecli, "2024-11-27")
    assert api.search(first, ecli, "2024-11-28")
    future = (datetime.date.fromisoformat(date) + datetime.timedelta(days=1)).isoformat()
    assert not api.search(first, ecli, future)
    print(json.dumps({"status": "passed", "ecli": ecli, "pages": 4,
        "checks": ["official PDF import", "URL restrictions", "deduplication", "session isolation",
                   "metadata", "whole decision and operative text", "exact page retrieval and PDF links",
                   "no invented point numbers", "date boundaries"]}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
