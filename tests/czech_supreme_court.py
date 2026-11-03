"""Live NS ČR HTML import against an isolated test deployment.

TEST_PASSWORD=test-only-password python3 -B tests/czech_supreme_court.py http://localhost:8093
"""
import json
import source_requests as api


def main():
    first, second = api.client(), api.client()
    kind = "czech_supreme_court_judgment"
    source = "https://rozhodnuti.nsoud.cz/Judikatura/judikatura_ns.nsf/WebSearch/A4D79508F15A7C04C1258A37004CF0D1?openDocument"
    ecli = "ECLI:CZ:NS:2023:33.CDO.314.2023.1"
    api.submit(first, kind, source, expected=401)
    for browser in (first, second):
        api.request(browser, "/api/session", "POST", {"password": api.PASSWORD})
    api.submit(first, kind, source, expected=403, headers={"Origin": "https://untrusted.example"})
    for invalid in ("33 Cdo 314/2023", ecli, source.replace("https:", "http:"),
                    source.replace("rozhodnuti.nsoud.cz", "example.com"), source + "&redirect=http://localhost"):
        api.submit(first, kind, invalid, expected=400)
    assert not api.listed(first)
    job = api.submit(first, kind, source.replace("rozhodnuti.nsoud.cz", "www.nsoud.cz"))
    assert job["reference"] == source
    assert api.submit(first, kind, source + "&Highlight=0,narovnani#text")["id"] == job["id"]
    assert not api.listed(second)
    assert api.submit(second, kind, source)["id"] == job["id"]
    result = api.completed(first, job["id"])
    assert result["imported_count"] == 1 and result["node_count"] > 3
    assert api.submit(first, kind, source)["status"] == "completed"
    versions = [v for v in api.request(first, "/api/corpus")["versions"]
                if (v.get("judgment") or {}).get("ecli") == ecli]
    assert len(versions) == 1
    version = versions[0]
    assert version["judgment"]["supporting_only"] is True
    assert version["judgment"]["language"] == "CS"
    assert version["judgment"]["decided_on"] == version["valid_from"] == "2023-08-17"
    assert version["source_url"] == source and not version["pdf_url"]
    date = version["verified_on"]
    for query, anchor in (("33 Cdo 314/2023", "judgment"),
                          (ecli + " výrok", "judgment-operative"),
                          ("33Cdo314/2023 odôvodnenie", "judgment-reasoning")):
        found = api.search(first, query, date)
        assert len(found) == 1 and found[0]["anchor"] == anchor, (query, found)
        assert found[0]["judgment"]["supporting_only"] is True
        assert found[0]["source_url"] == source
        stored = api.request(first, f"/api/nodes/{found[0]['id']}")
        assert stored["judgment"] == found[0]["judgment"]
    for query in ("33 Cdo 315/2023", ecli + "0", "33 Cdo 314/2023 bod 2", "33 Cdo 314/2023 strana 2"):
        assert not api.search(first, query, date), query
    assert not api.search(first, ecli, "2023-08-16")
    assert api.search(first, ecli, "2023-08-17")
    assert "Poučení:" in api.search(first, ecli, date)[0]["text"]
    print(json.dumps({"status": "passed", "ecli": ecli, "nodes": result["node_count"],
        "checks": ["live official import", "URL restrictions", "authentication and origin",
                   "deduplication", "session isolation", "supporting role", "Czech original",
                   "exact retrieval", "no invented points or PDF", "date boundaries"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
