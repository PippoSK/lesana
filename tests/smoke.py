"""Real PostgreSQL + HTTP checks. Uses only Python's standard library.

python3 tests/smoke.py http://localhost:8088
TEST_PASSWORD=test-only-password python3 tests/smoke.py http://localhost:8089 agent
Every conversation created here is deleted, even after failure.
"""
import datetime
import http.cookiejar
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from zoneinfo import ZoneInfo

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8088"
AGENT = len(sys.argv) > 2 and sys.argv[2] == "agent"
PASSWORD = os.environ.get("TEST_PASSWORD", "")


def client():
    return urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))


def request(browser, path, method="GET", data=None, expected=200, extra_headers=None):
    headers = {"Content-Type": "application/json", **(extra_headers or {})}
    req = urllib.request.Request(BASE + path, method=method, headers=headers,
                                 data=json.dumps(data).encode() if data is not None else None)
    try:
        with browser.open(req, timeout=30) as response:
            body = response.read().decode()
            assert response.status == expected, (response.status, body)
            if "text/event-stream" in response.headers.get("Content-Type", ""):
                events = []
                for frame in body.replace("\r\n", "\n").split("\n\n"):
                    kind, payload = None, None
                    for line in frame.splitlines():
                        if line.startswith("event:"): kind = line[6:].strip()
                        if line.startswith("data:"): payload = json.loads(line[5:])
                    if kind: events.append((kind, payload))
                return events
            return json.loads(body) if body else None
    except urllib.error.HTTPError as error:
        assert error.code == expected, (error.code, error.read().decode())
        body = error.read().decode()
        try: return json.loads(body)
        except json.JSONDecodeError: return {"error": body}


first, second = client(), client()
created = []
checks = []
try:
    assert request(first, "/health")["status"] == "ok"
    request(first, "/api/conversations", expected=401)
    if PASSWORD:
        request(first, "/api/session", "POST", {"password": "incorrect"}, expected=401)
    for browser in (first, second):
        request(browser, "/api/session", "POST", {"password": PASSWORD})
    request(first, "/api/conversations", "POST", {}, expected=403,
            extra_headers={"Origin": "https://untrusted.example"})
    checks.append("authentication and cross-origin write protection")
    corpus = request(first, "/api/corpus")
    versions = {v["number"]: v for v in corpus["versions"]}
    version = versions["162/2015 Z. z."]
    assert (version["sections"], version["paragraphs"], version["letters"], version["points"]) == (523, 971, 462, 2), version
    for number, counts in [("160/2015 Z. z.", (483, 919, 233, 18, 1793)),
                           ("161/2015 Z. z.", (404, 725, 135, 14, 1365)),
                           ("71/1967 Zb.", (85, 226, 26, 0, 366)),
                           ("372/1990 Zb.", (90, 237, 228, 0, 656)),
                           ("300/2005 Z. z.", (528, 1565, 1793, 0, 4072)),
                           ("301/2005 Z. z.", (643, 2127, 636, 0, 3621)),
                           ("40/1964 Zb.", (687, 1382, 192, 0, 2526)),
                           ("108/2024 Z. z.", (133, 731, 584, 25, 2143))]:
        v = versions[number]
        assert tuple(v[k] for k in ("sections", "paragraphs", "letters", "articles", "nodes")) == counts, v
    assert corpus["agent_enabled"] == AGENT
    date = corpus["default_date"]
    assert date == datetime.datetime.now(ZoneInfo("Europe/Bratislava")).date().isoformat()
    assert request(first, "/api/search?q=SSP")["as_of"] == date
    before_corpus = (datetime.date.fromisoformat(min(v["valid_from"] for v in corpus["versions"])) - datetime.timedelta(days=1)).isoformat()
    checks.append("complete corpus and verified version metadata")

    def search(query, as_of=date):
        params = urllib.parse.urlencode({"q": query, "as_of": as_of})
        return request(first, "/api/search?" + params)

    treaty = versions['Bruselský dohovor (1968)']
    assert treaty['treaty']['language'] == 'en'
    assert treaty['treaty']['published_on'] == '1998-01-26'
    assert (treaty['articles'], treaty['nodes']) == (96, 100)
    for alias in ('Bruselský dohovor', 'BD', 'CELEX:41998A0126', '41998A0126'):
        found = search(alias)['sources']
        assert len(found) == 1 and found[0]['kind'] == 'treaty', (alias, found)
        assert found[0]['treaty'] and not found[0]['judgment']
        assert '#' not in found[0]['source_url'] and 'zákona č.' not in found[0]['citation']
    for query, anchor, phrase in [
        ('čl. 13 Bruselského dohovoru', 'predpis.text2.clanok-13', 'outside his trade or profession'),
        ('čl. 14 BD', 'predpis.text2.clanok-14', 'counter-claim'),
        ('Article 6a Brussels Convention', 'predpis.text2.clanok-6a', 'limitation of such liability'),
        ('čl. Vd protokolu BD', 'treaty-protocol.clanok-vd', 'European Patent Office'),
        ('čl. 13 protokolu o výklade BD', 'treaty-interpretation.clanok-13', 'revision of this Protocol'),
    ]:
        found = search(query)['sources']
        assert len(found) == 1 and found[0]['anchor'] == anchor, (query, found)
        assert phrase in found[0]['text'], (query, found)
        assert found[0]['retrieval_methods'] == ['exact']
    assert len(search('čl. 13, 14 a 15 BD')['sources']) == 3
    assert not search('čl. 13 BD', '1998-01-25')['sources']
    assert search('čl. 13 BD', '1998-01-26')['sources']
    assert search('čl. 13 BD', '2026-10-04')['sources'][0]['verified_on'] == treaty['verified_on']
    assert not search('čl. 13 ods. 99 BD')['sources']
    regulation = next((v for v in corpus['versions'] if (v.get('regulation') or {}).get('celex') == '32012R1215'), None)
    regulation_sources = search('čl. 13 nariadenia 1215/2012')['sources']
    if regulation and regulation['valid_from'] <= date:
        assert len(regulation_sources) == 1 and regulation_sources[0]['regulation']['celex'] == '32012R1215'
        assert regulation_sources[0]['anchor'] == 'predpis.text2.clanok-13' and not regulation_sources[0]['treaty']
    else:
        assert not regulation_sources
    assert any(s['anchor'] == 'predpis.text2.clanok-13' for s in search('Aká je právomoc pri spotrebiteľských zmluvách podľa Bruselského dohovoru?')['sources'])
    checks.append('treaty articles, separate protocols, publication dates and source identity')

    judgment = versions['C-464/01']
    assert judgment['judgment']['ecli'] == 'ECLI:EU:C:2005:32'
    assert judgment['judgment']['decided_on'] == '2005-01-20'
    assert judgment['points'] == 56 and judgment['nodes'] == 58
    for alias in ('C-464/01', 'C‑464/01', 'Gruber', 'Johann Gruber proti Bay Wa AG', 'ECLI:EU:C:2005:32', '62001CJ0464'):
        found = search(alias)['sources']
        assert len(found) == 1 and found[0]['kind'] == 'judgment', (alias, found)
        assert 'zákona č.' not in found[0]['citation'] and '#' not in found[0]['source_url']
        assert found[0]['retrieval_methods'] == ['exact']
    points = search('C-464/01 body 39–41, 46 a 54')['sources']
    assert [s['anchor'] for s in points] == [f'judgment-point-{n}' for n in (39, 40, 41, 46, 54)]
    assert 'zanedbateľnú úlohu' in points[0]['text']
    assert 'oprávnene vzbudiť dojem' in points[-1]['text']
    assert search('Gruber výrok')['sources'][0]['kind'] == 'judgment_operative'
    assert not search('Gruber bod 999')['sources']
    assert not search('C-999/01 Gruber')['sources']
    assert not search('ECLI:EU:C:2005:320 Gruber')['sources']
    request(first, '/api/search?' + urllib.parse.urlencode({'q': 'Gruber body 1 až 56'}), expected=400)
    assert not search('C-464/01', '2005-01-19')['sources']
    assert search('C-464/01', '2005-01-20')['sources']
    assert search('C-464/01', '2026-10-03')['sources']
    assert any(s['judgment'] for s in search('zanedbateľný podnikateľský účel spotrebiteľská zmluva')['sources'])
    checks.append('judgment metadata, exact points, aliases, fulltext and date boundaries')

    exact = search("§ 3 ods. 1 písm. b) SSP")["sources"]
    assert len(exact) == 1 and exact[0]["anchor"] == "paragraf-3.odsek-1.pismeno-b", exact
    assert exact[0]["retrieval_methods"] == ["exact"], exact
    assert "Na účely tohto zákona sa rozumie" in exact[0]["context"]
    assert "#paragraf-3.odsek-1.pismeno-b" in exact[0]["source_url"]
    assert search("§ 3 ods. 1 pism. b) SSP")["sources"][0]["id"] == exact[0]["id"]
    assert "ods. 1" not in search("§ 1 písm. a) SSP")["sources"][0]["citation"]
    assert "dvoch mesiacov" in search("§ 181 SSP")["sources"][0]["text"]
    ranked = search("Aká je lehota na podanie správnej žaloby proti rozhodnutiu orgánu verejnej správy?")["sources"]
    assert ranked[0]["anchor"] == "paragraf-181.odsek-1", ranked
    representation = search("Kedy musím byť zastúpený advokátom?")["sources"]
    assert any(s["anchor"] == "paragraf-49.odsek-1" for s in representation[:6]), representation
    assert search("advokat")["sources"]
    assert search("nečinnosť")["sources"]
    assert not search("§ 181 SSP", "2025-01-01")["sources"]
    assert not search("§ 1", before_corpus)["covered"]
    assert search("§ 181 SSP", "2026-10-13")["sources"][0]["verified_on"] == version["verified_on"]
    assert not search("§ 1 TP", "2028-01-01")["sources"]
    request(first, "/api/search?" + urllib.parse.urlencode({"q": "x", "as_of": "not-a-date"}), expected=400)
    assert {s["act_title"] for s in search("§ 1")["sources"]} == {
        "Civilný sporový poriadok", "Civilný mimosporový poriadok", "Správny súdny poriadok",
        "Zákon o správnom konaní (správny poriadok)",
        "Zákon Slovenskej národnej rady o priestupkoch", "Trestný zákon", "Trestný poriadok",
        "Občiansky zákonník", "Zákon o ochrane spotrebiteľa a o zmene a doplnení niektorých zákonov"}
    for scope, title, date_from in [("CSP", "Civilný sporový poriadok", "2026-07-15"),
                                   ("CMP", "Civilný mimosporový poriadok", "2026-08-17")]:
        sources = search("§ 1 " + scope)["sources"]
        assert len(sources) == 1 and sources[0]["act_title"] == title, sources
        assert sources[0]["valid_from"] == date_from
        principle = search("čl. 2 ods. 1 " + scope)["sources"]
        assert len(principle) == 1 and principle[0]["citation"].startswith("Čl. 2 ods. 1"), principle
        assert principle[0]["anchor"] == "predpis.text2.clanok-2.odsek-1"
        assert principle[0]["act_title"] == title
        assert search("§ 1 " + scope, date_from)["sources"]
        assert not search("§ 1 " + scope, "2026-07-14")["sources"]
    assert not search("§ 1 CMP", "2026-08-16")["sources"]
    for scope in ("SP", "Správneho poriadku", "spravneho poriadku", "zákona o správnom konaní", "zákona č. 71/1967 Zb."):
        sources = search("§ 54 ods. 2 " + scope)["sources"]
        assert len(sources) == 1 and sources[0]["act_title"] == "Zákon o správnom konaní (správny poriadok)", sources
        assert sources[0]["citation"] == "§ 54 ods. 2 zákona č. 71/1967 Zb."
        assert "15 dní" in sources[0]["text"]
        assert sources[0]["source_url"].endswith("/1967/71/20180901.html#paragraf-54.odsek-2")
        assert sources[0]["pdf_url"].endswith("ZZ_1967_71_20180901.pdf")
    assert search("§ 1 SP", "2018-09-01")["sources"]
    assert not search("§ 1 SP", "2018-08-31")["sources"]
    assert search("§ 1 SP", "2026-10-02")["sources"]
    assert not search("§ 181 SP")["sources"]
    letter = search("§ 30 ods. 1 písm. a) SP")["sources"][0]
    assert "Správny orgán konanie zastaví" in letter["context"] and "j)" in letter["context"]
    assert all("71/1967" in s["citation"] for s in search("odvolanie SP")["sources"])
    for aliases, number, title, date_from, provision, phrase in [
        (("ZoP", "zákona o priestupkoch", "priestupkového zákona"), "372/1990 Zb.", "Zákon Slovenskej národnej rady o priestupkoch", "2026-09-01", "§ 2 ods. 1", "Priestupkom je zavinené konanie"),
        (("TZ", "Trestného zákona", "trestneho zakona"), "300/2005 Z. z.", "Trestný zákon", "2026-08-18", "§ 212 ods. 1 písm. a)", "malú škodu"),
        (("TP", "Trestného poriadku", "trestneho poriadku"), "301/2005 Z. z.", "Trestný poriadok", "2026-08-18", "§ 34 ods. 1", "právo odoprieť vypovedať"),
    ]:
        for alias in (*aliases, "zákona č. " + number):
            sources = search(provision + " " + alias)["sources"]
            assert len(sources) == 1 and sources[0]["act_title"] == title, sources
            assert sources[0]["citation"] == provision + " zákona č. " + number
            assert phrase in sources[0]["text"] and sources[0]["valid_from"] == date_from
        assert search("§ 1 " + aliases[0], date_from)["sources"]
        assert not search("§ 1 " + aliases[0], "2026-08-17")["sources"]
        assert search("§ 1 " + aliases[0], "2026-10-02")["sources"]
        assert all(s["act_title"] == title for s in search("zavinenie " + aliases[0])["sources"])
    assert not search("§ 1 ZoP", "2026-08-31")["sources"]
    assert search("§ 1 TP")["sources"][0]["valid_to"] == "2027-12-31"
    for query, anchor in [("§ 88aa ZoP", "paragraf-88aa"), ("§ 305aa TZ", "paragraf-305aa")]:
        sources = search(query)["sources"]
        assert len(sources) == 1 and sources[0]["anchor"] == anchor, sources
    criminal_letter = search("§ 212 ods. 1 písm. a) TZ")["sources"][0]
    assert "Kto si prisvojí cudziu vec" in criminal_letter["context"]
    assert "potrestá sa" in criminal_letter["context"]
    for alias in ("OZ", "Občianskeho zákonníka", "obcianskeho zakonnika", "zákona č. 40/1964 Zb."):
        sources = search("§ 420 ods. 1 " + alias)["sources"]
        assert len(sources) == 1 and sources[0]["act_title"] == "Občiansky zákonník", sources
        source = sources[0]
        assert source["citation"] == "§ 420 ods. 1 zákona č. 40/1964 Zb."
        assert "porušením právnej povinnosti" in source["text"]
        assert source["source_url"].endswith("/1964/40/20260731.html#paragraf-420.odsek-1")
        assert source["pdf_url"].endswith("ZZ_1964_40_20260731.pdf")
        assert source["valid_from"] == "2026-07-31" and source["verified_on"] == "2026-10-02"
    assert search("§ 1 OZ", "2026-07-31")["sources"]
    assert search("§ 1 OZ", "2026-10-02")["sources"]
    assert not search("§ 1 OZ", "2026-07-30")["sources"]
    assert search("§ 1 OZ", "2026-10-03")["sources"]
    assert search("§ 879aa OZ")["sources"][0]["anchor"] == "paragraf-879aa"
    assert not search("§ 420 ods. 999 OZ")["sources"]
    civil_letter = search("§ 53 ods. 4 písm. a) OZ")["sources"][0]
    assert "Za neprijateľné podmienky" in civil_letter["context"] and "b)" in civil_letter["context"]
    assert search("§ 11 OZ")["sources"][0]["citation"] == "§ 11 zákona č. 40/1964 Zb."
    civil_hits = search("ochrana osobnosti OZ")["sources"]
    assert civil_hits and all(s["act_title"] == "Občiansky zákonník" for s in civil_hits)
    for alias in ("ZoS", "zákona o ochrane spotrebiteľa", "zakona o ochrane spotrebitela", "zákona č. 108/2024 Z. z."):
        sources = search("§ 20 ods. 1 písm. a) " + alias)["sources"]
        assert len(sources) == 1 and "108/2024" in sources[0]["citation"], sources
        source = sources[0]
        assert source["citation"] == "§ 20 ods. 1 písm. a) zákona č. 108/2024 Z. z."
        assert "14 dní" in source["text"] and "30 dní" in source["context"]
        assert source["source_url"].endswith("/2024/108/20260927.html#paragraf-20.odsek-1.pismeno-a")
        assert source["pdf_url"].endswith("ZZ_2024_108_20260927.pdf")
        assert source["valid_from"] == "2026-09-27" and source["verified_on"] == "2026-10-02"
    assert search("§ 1 ZoS", "2026-09-27")["sources"]
    assert search("§ 1 ZoS", "2026-10-02")["sources"]
    assert not search("§ 1 ZoS", "2026-09-26")["sources"]
    assert search("§ 1 ZoS", "2026-10-03")["sources"]
    assert not search("§ 619 ZoS")["sources"]
    assert not search("§ 619 ods. 1 ZoS")["sources"]
    assert not search("§ 3 zákona o ochrane spotrebiteľa č. 250/2007")["sources"]
    assert search("§ 619 OZ")["sources"]
    assert search("§ 20a ZoS")["sources"][0]["anchor"] == "paragraf-20a"
    consumer_hits = search("zníženie ceny tovaru ZoS")["sources"]
    assert consumer_hits and all("108/2024" in s["citation"] and "citované znenie" not in s["citation"] for s in consumer_hits)
    assert not search("čl. 2 SSP")["sources"]
    assert "môže súd rozhodnúť" in search("§ 274 CSP")["sources"][0]["text"]
    assert "v lehote 15 dní" in search("§ 362 zákona č. 160/2015")["sources"][0]["text"]
    assert "dedičstve" in search("§ 158 CMP")["sources"][0]["text"]
    assert all(s["act_title"] == "Civilný mimosporový poriadok" for s in search("dedičstvo CMP")["sources"])
    assert any(s["anchor"].startswith("predpis.text2.clanok-") for s in search("zjavné zneužitie práva CSP")["sources"])
    for query, expected in [
        ("§ 20 ods. 1 ZoS a § 619 OZ", {("paragraf-20.odsek-1", "108/2024"), ("paragraf-619", "40/1964")}),
        ("§ 420 ods. 1 OZ a § 420 CSP", {("paragraf-420.odsek-1", "40/1964"), ("paragraf-420", "160/2015")}),
        ("§ 54 SP a § 181 SSP", {("paragraf-54", "71/1967"), ("paragraf-181", "162/2015")}),
        ("§ 181 a 182 SSP", {("paragraf-181", "162/2015"), ("paragraf-182", "162/2015")}),
        ("§ 54 ods. 2 SP a § 362 ods. 1 CSP", {("paragraf-54.odsek-2", "71/1967"), ("paragraf-362.odsek-1", "160/2015")}),
        ("čl. 2 CSP a čl. 2 CMP", {("predpis.text2.clanok-2", "160/2015"), ("predpis.text2.clanok-2", "161/2015")}),
    ]:
        found = search(query)["sources"]
        assert len(found) == len(expected), (query, found)
        assert all(any(s["anchor"] == anchor and act in s["citation"] for s in found) for anchor, act in expected), (query, found)
    assert not search("§ 181 ods. 999 SSP")["sources"]
    assert not search("§ 181 ods. 1 písm. z) SSP")["sources"]
    assert len(search("§ 181 SSP a § 181 SSP")["sources"]) == 1
    checks.append("exact letters, civil principles, act disambiguation, Slovak search and date boundaries")
    node = request(first, f"/api/nodes/{exact[0]['id']}")
    assert node["citation"] == exact[0]["citation"]

    conversation = request(first, "/api/conversations", "POST", {})["id"]
    created.append(conversation)
    request(second, f"/api/conversations/{conversation}", expected=404)
    request(second, f"/api/conversations/{conversation}/chat", "POST", {"message": "test"}, expected=404)
    request(second, f"/api/conversations/{conversation}", "DELETE", expected=404)
    request(first, f"/api/conversations/{conversation}/chat", "POST", {"message": " "}, expected=400)
    checks.append("conversation ownership and input validation")
    events = request(first, f"/api/conversations/{conversation}/chat", "POST",
                     {"message": "Aká je lehota podľa § 181?"})
    assert any(kind == "status" for kind, _ in events), events
    answers = [payload for kind, payload in events if kind == "answer"]
    assert len(answers) == 1, events
    answer = answers[0]
    assert answer["as_of"] == date, answer
    assert answer["sources"] and answer["mode"] == ("agent" if AGENT else "research"), answer
    assert any(s["anchor"] == "paragraf-181" and "exact" in s["retrieval_methods"] for s in answer["sources"]), answer
    assert all(s["retrieval_methods"] and set(s["retrieval_methods"]) <= {"exact", "related"} for s in answer["sources"]), answer
    messages = request(first, f"/api/conversations/{conversation}")
    assert len(messages) == 2 and messages[1]["sources"] == answer["sources"]
    assert all(message["as_of"] == date for message in messages)
    if AGENT:
        assert "[N" in answer["answer"]
        assert answer["claims"] and answer["claims"] == messages[1]["claims"]
        for claim in answer["claims"]:
            for evidence in claim["evidence"]:
                source = next(s for s in answer["sources"] if s["id"] == evidence["source_id"])
                assert evidence["quote"] in source["text"] or evidence["quote"] in source["context"]
    checks.append("SSE lifecycle, source persistence and conversation history")
    if not AGENT:
        case_conversation = request(first, '/api/conversations', 'POST', {})['id']
        created.append(case_conversation)
        case_events = request(first, f'/api/conversations/{case_conversation}/chat', 'POST', {'message': 'Gruber bod 39'})
        case_answer = next(payload for kind, payload in case_events if kind == 'answer')
        assert {s['kind'] for s in case_answer['sources']} == {'judgment_point', 'judgment'}
        case_history = request(first, f'/api/conversations/{case_conversation}')
        assert case_history[-1]['sources'] == case_answer['sources']
        checks.append('judgment context and metadata persist through chat')
    for legacy_date in (before_corpus, "2099-01-01"):
        events = request(first, f"/api/conversations/{conversation}/chat", "POST",
                         {"message": "Aká je lehota podľa § 181 SSP?", "as_of": legacy_date})
        followup = next(payload for kind, payload in events if kind == "answer")
        assert followup["as_of"] == date and followup["sources"], followup
        history = request(first, f"/api/conversations/{conversation}")
        assert all(message["as_of"] == date for message in history[-2:])
    checks.append("current date on new and continued turns; legacy chat dates ignored")
    if AGENT:
        for trigger, anchor, law in [
            ("CONTEXT_INITIAL § 216 ods. 2 písm. c) TZ", "paragraf-216", "300/2005"),
            ("CONTEXT_TOOL", "paragraf-216", "300/2005"),
            ("CONTEXT_REFERENCE § 62 ods. 1 písm. d) SP", "paragraf-9", "71/1967"),
            ("CONTEXT_EXTERNAL § 9 ods. 2 TP", "paragraf-326a", "300/2005"),
            ("CONTEXT_SECTION", "paragraf-326a", "300/2005"),
            ("CONTEXT_GAP § 216 ods. 2 písm. c) TZ", "paragraf-125", "300/2005"),
        ]:
            regression = request(first, "/api/conversations", "POST", {})["id"]
            created.append(regression)
            architecture = "pipeline" if "CONTEXT_GAP" in trigger else "agentic"
            events = request(first, f"/api/conversations/{regression}/chat", "POST", {"message": trigger, "architecture": architecture})
            result = next((p for kind, p in events if kind == "answer"), None)
            assert result and result["mode"] == "agent", events
            source = next(s for s in result["sources"] if s["anchor"] == anchor and law in s["citation"])
            assert source["retrieval_methods"] == ["related"], source
            saved = request(first, f"/api/conversations/{regression}")[-1]
            assert saved["sources"] == result["sources"] and saved["claims"] == result["claims"]
            if "CONTEXT_GAP" in trigger:
                assert "chýba" not in result["answer"]
                assert any(k == "status" and "Dohľadávam" in p["text"] for k, p in events)
            if "CONTEXT_INITIAL" in trigger:
                follow = request(first, f"/api/conversations/{regression}/chat", "POST", {"message": "A čo z toho vyplýva?"})
                assert next(p for k, p in follow if k == "answer")["mode"] == "agent", follow
        checks.append("automatic parent and linked context in initial, tool, follow-up and evidence catalog; provenance persistence")
        for trigger, code in [("PROVIDER_ERROR § 181", "model_unavailable"), ("PROVIDER_AUTH § 181", "model_configuration"), ("REVIEW_OFFLINE § 181", "model_unavailable"), ("REVIEW_ALWAYS_TRUNCATED § 181", "model_output_limit"), ("DRAFT_OUTPUT_LIMIT § 181", "model_output_limit"), ("MISSING_REVIEW § 181", "model_response"), ("MALFORMED_REVIEW § 181", "model_response"), ("PARTIAL_MISSING_REVIEW § 181", "model_response"), ("MALFORMED_DRAFT § 181", "model_response"), ("INVALID_DRAFT_SCHEMA § 181", "model_response")]:
            invalid = request(first, "/api/conversations", "POST", {})["id"]
            created.append(invalid)
            events = request(first, f"/api/conversations/{invalid}/chat", "POST", {"message": trigger})
            errors = [payload for kind, payload in events if kind == "error"]
            assert len(errors) == 1 and errors[0]["code"] == code, events
            assert not any(kind == "answer" for kind, _ in events), events
            assert request(first, f"/api/conversations/{invalid}") == []
        for trigger in ("INVALID_CITATION § 181", "INVALID_PASSAGE § 181", "FABRICATED_QUOTE § 181", "UNSUPPORTED_CLAIM § 181", "REPAIR_DRAFT_FORMAT UNSUPPORTED_CLAIM § 181"):
            fallback = request(first, "/api/conversations", "POST", {})["id"]
            created.append(fallback)
            events = request(first, f"/api/conversations/{fallback}/chat", "POST", {"message": trigger})
            answers = [payload for kind, payload in events if kind == "answer"]
            assert len(answers) == 1 and answers[0]["mode"] == "research_fallback", events
            assert answers[0]["sources"] and not answers[0]["claims"]
            assert "Nepodložený" not in answers[0]["answer"] and "Vymyslený" not in answers[0]["answer"]
            assert not any(kind == "error" for kind, _ in events), events
            saved = request(first, f"/api/conversations/{fallback}")
            assert len(saved) == 2 and saved[1]["mode"] == "research_fallback" and not saved[1]["claims"]
            assert saved[1]["sources"] == answers[0]["sources"]
        for trigger in ("PARTIAL_SUPPORT § 181", "PARTIAL_INVALID_REPAIR § 181", "PARTIAL_FORMAT_REPAIR § 181", "UNSAFE_QUESTION § 181"):
            partial = request(first, "/api/conversations", "POST", {})["id"]
            created.append(partial)
            events = request(first, f"/api/conversations/{partial}/chat", "POST", {"message": trigger})
            result = next(payload for kind, payload in events if kind == "answer")
            assert result["mode"] == "agent_partial" and len(result["claims"]) == 1, result
            assert "Testovacia odpoveď" in result["answer"] and "podloženú časť" in result["answer"], result
            assert "Nepodložený vedľajší záver" not in result["answer"], result
            assert "Nepodložená otázka" not in result["answer"], result
            assert result["sources"][0]["retrieval_methods"] == ["exact"], result
            saved = request(first, f"/api/conversations/{partial}")[-1]
            assert saved["mode"] == result["mode"] and saved["content"] == result["answer"]
            assert saved["claims"] == result["claims"] and saved["sources"] == result["sources"]
        for trigger, expected_mode in [("REPAIR_DRAFT_FORMAT § 181", "agent"), ("REPAIR_DRAFT_FORMAT REPAIR_SUPPORT § 181", "agent"), ("FENCED_DRAFT § 181", "agent"), ("REPAIR_QUOTE § 181", "agent"), ("REPAIR_SUPPORT § 181", "agent"), ("REPAIR_REVIEW § 181", "agent"), ("REVIEW_OUTPUT_LIMIT § 181", "agent"), ("SINGLE_REVIEW_BUDGET § 181", "agent"), ("BATCHED_REVIEW § 181", "agent_partial"), ("EMPTY_QUESTION_FALSE § 181", "agent"), ("CLARIFICATION § 181", "clarification")]:
            repaired = request(first, "/api/conversations", "POST", {})["id"]
            created.append(repaired)
            events = request(first, f"/api/conversations/{repaired}/chat", "POST", {"message": trigger})
            result = [payload for kind, payload in events if kind == "answer"]
            assert len(result) == 1 and result[0]["mode"] == expected_mode, events
            if expected_mode == "clarification": assert not result[0]["claims"] and not result[0]["sources"]
            if "REVIEW_OUTPUT_LIMIT" in trigger: assert len(result[0]["claims"]) == 3, result
            if "BATCHED_REVIEW" in trigger:
                assert [c["text"] for c in result[0]["claims"]] == [f"Podložený bod {i}." for i in (0, 2, 3, 4, 5)], result
                assert "Nepodložený" not in result[0]["answer"], result
        checks.append("scoped tools; passage selection; safe research fallback and persistence; targeted repair; provider error codes; clarification")
    print("PASS:", *checks, sep="\n  - ")
finally:
    for conversation in created:
        request(first, f"/api/conversations/{conversation}", "DELETE", expected=204)
