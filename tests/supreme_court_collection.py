"""Import the scanned 2/2002 issue through the real background API.

TEST_PASSWORD=test-only-password python3 -B tests/supreme_court_collection.py http://localhost:8089
"""
import json
import time

import source_requests as api

URL = "https://www.nsud.sk/data/att/67c/688261.8f5b7b.pdf"
EXPECTED = {
    20: (9, "2000-12-14"), 21: (10, "2001-05-17"),
    22: (14, "1999-12-07"), 23: (17, "2001-05-23"),
    24: (27, "2000-09-27"), 25: (31, "2001-07-24"),
    26: (33, "2001-01-30"), 27: (36, "2000-08-24"),
    28: (39, "2001-07-04"), 29: (43, "2000-11-28"),
    30: (49, "2000-11-30"), 31: (55, "2001-08-14"),
    32: (58, "2001-09-26"), 33: (63, "2001-09-27"),
    34: (68, "2001-10-23"), 35: (72, "2001-03-27"),
}


def main():
    first, second = api.client(), api.client()
    for browser in (first, second):
        api.request(browser, "/api/session", "POST", {"password": api.PASSWORD})
    kind = "supreme_court_collection"
    for invalid in ("2/2002", URL.replace("www.nsud.sk", "example.com"), URL + "?redirect=http://localhost"):
        api.submit(first, kind, invalid, expected=400)
    job = api.submit(first, kind, URL)
    assert api.submit(first, kind, URL + "#page=9")["id"] == job["id"]
    assert not api.listed(second)
    deadline = time.monotonic() + 1200
    while True:
        result = api.listed(first)[job["id"]]
        assert result["status"] != "failed", result
        if result["status"] == "completed":
            break
        assert time.monotonic() < deadline, result
        time.sleep(2)
    assert result["imported_count"] == 16, result
    assert api.submit(first, kind, URL)["status"] == "completed"
    assert api.submit(second, kind, URL.replace("www.nsud.sk", "nsud.sk"))["id"] == job["id"]
    versions = [v for v in api.request(first, "/api/corpus")["versions"]
                if ((v.get("judgment") or {}).get("collection") or {}).get("issue") == "2/2002"]
    assert len(versions) == 16, len(versions)
    assert {v["judgment"]["collection"]["number"] for v in versions} == set(range(20, 36))
    assert sum(v["nodes"] for v in versions) == result["node_count"]
    for version in versions:
        judgment = version["judgment"]
        collection = judgment["collection"]
        assert (collection["first_page"], judgment["decided_on"]) == EXPECTED[collection["number"]], judgment
        assert collection["ocr"] and not judgment["ecli"], judgment
        assert version["source_url"] == version["pdf_url"] == URL
        date = version["verified_on"]
        citation = f"R {collection['number']}/2002"
        whole = api.search(first, citation, date)
        assert len(whole) == 1 and whole[0]["kind"] == "judgment", (citation, whole)
        assert "OCR" in whole[0]["judgment"]["legal_context"]
        page = collection["first_page"]
        found = api.search(first, f"{citation} strana {page}", date)
        assert len(found) == 1 and found[0]["source_url"] == URL + f"#page={page}", (citation, found)
        headnote = api.search(first, citation + " právna veta", date)
        assert len(headnote) == 1 and headnote[0]["anchor"] == "judgment-headnote", (citation, headnote)
        assert not api.search(first, citation + " výrok", date), "Do not invent an operative part"
        assert not api.search(first, citation + " strana 999", date)
    date = versions[0]["verified_on"]
    for missing in ("R 19/2002", "R 36/2002", "R 120/2002"):
        assert not api.search(first, missing, date), missing
    print(json.dumps({"status": "passed", "issue": "2/2002", "imported_count": 16,
                      "node_count": result["node_count"], "checks": ["scanned PDF over 20 MB", "all 16 entries",
                      "OCR provenance", "exact R citations and headnotes", "absolute PDF page links",
                      "no invented ECLI or operative", "session isolation", "idempotent request"]}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
