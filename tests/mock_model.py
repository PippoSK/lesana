"""Deterministic protocol fixture; never used as a real legal model."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import time


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/v1/embeddings":
            self.embeddings(request)
            return
        request = self.normalize_request(request)
        messages = request["messages"]
        # Rig uses the standard multipart form even for text-only messages.
        for message in messages:
            if isinstance(message.get("content"), list):
                message["content"] = "".join(part.get("text", "") for part in message["content"])
        text = json.dumps(messages, ensure_ascii=False)
        budget = request.get("max_completion_tokens", request.get("max_tokens", 0))
        assert "TRUNCATED_DRAFT_SENTINEL" not in text, "A truncated answer must be discarded before retry"
        self.assert_conversation_history(request)
        if self.respond_failure(request):
            return
        finish_reason = "stop"
        assert '"retrieval_methods"' not in text.replace('\\"', '"'), "UI retrieval metadata must not enter model prompts"
        if "PROVIDER_PAYMENT" in text or ("REVIEW_PAYMENT" in text and messages[0]["content"].startswith("Si kontrolór")):
            self.send_response(402)
            self.end_headers()
            return
        if "PROVIDER_ERROR" in text or "PROVIDER_AUTH" in text or ("REVIEW_OFFLINE" in text and messages[0]["content"].startswith("Si kontrolór")):
            self.send_response(401 if "PROVIDER_AUTH" in text else 503)
            self.end_headers()
            return
        if messages[0]["content"].startswith("Si kontrolór"):
            draft = json.loads(messages[1]["content"])
            review_group_size = 1 if request.get("model") == "deepseek/deepseek-v4.1-flash" else 3
            assert len(draft["claims"]) <= review_group_size, "Review requests must be bounded by model policy"
            verdicts = [{"index": i, "supported": "Nepodložený" not in c["text"], "reason": "Chýba opora pre celý záver." if "Nepodložený" in c["text"] else ""} for i, c in enumerate(draft["claims"])]
            if "MISSING_REVIEW" in text: verdicts = []
            if "REPAIR_REVIEW" in text and "review_format_feedback" not in text: verdicts = []
            result = {"claims": verdicts, "questions_safe": True}
            if "UNSAFE_QUESTION" in text or "EMPTY_QUESTION_FALSE" in text:
                result["questions_safe"] = False
            message = {"role": "assistant", "content": json.dumps(result)}
            if "MALFORMED_REVIEW" in text: message["content"] = "{"
            initial_review_budget = 24000 if request.get("model") in ("deepseek/deepseek-v4.1-flash", "gpt-6.1-sol") else 12000
            if "SINGLE_REVIEW_HIGH_BUDGET" in text and budget <= initial_review_budget:
                # Valid-looking negative verdicts must not leak out of a truncated review.
                result["claims"] = [{"index": i, "supported": False, "reason": "Truncated review must be discarded."} for i in range(len(draft["claims"]))]
                message["content"] = json.dumps(result)
                finish_reason = "length"
            if ("REVIEW_OUTPUT_LIMIT" in text and len(draft["claims"]) > 1) or ("SINGLE_REVIEW_BUDGET" in text and budget <= 6000) or "REVIEW_ALWAYS_TRUNCATED" in text:
                # Even valid-looking JSON must not be accepted after truncation.
                finish_reason = "length"
        elif "RIG_" in text:
            message = self.rig_fixture(request, text)
        elif "CONTEXT_" in text:
            message = self.context_fixture(request, text)
        elif "tools" in request:
            if not any(m["role"] == "tool" for m in messages):
                message = {"role": "assistant", "content": None, "tool_calls": [{"id": "read_section", "type": "function", "function": {"name": "get_section", "arguments": '{"section":"181","act":"SSP"}'}}]}
            else:
                message = {"role": "assistant", "content": "Ready to answer."}
        else:
            sources = self.sources(messages)
            source = next(s for s in sources if s["anchor"] == "paragraf-181" and s["act_title"] == "Správny súdny poriadok")
            source_id = 999999999 if "INVALID_CITATION" in text else source["id"]
            # Protocol fixture only: this is not an assessment of legal reasoning.
            claim_text = "Testovacia odpoveď: overený paragraf o lehote."
            repairing = any("Odpoveď neprešla kontrolou" in m.get("content", "") for m in messages if isinstance(m.get("content"), str))
            catalogs = []
            for m in messages:
                try:
                    value = json.loads(m.get("content", ""))
                    if isinstance(value, dict): catalogs.extend(value.get("evidence_catalog", []))
                except (ValueError, TypeError): pass
            entry = next(item for item in catalogs if item["source_id"] == source["id"])
            evidence = {"source_id": source_id, "passage_id": entry["passages"][0]["id"]}
            if "INVALID_PASSAGE" in text: evidence["passage_id"] = 999999
            if "FABRICATED_QUOTE" in text or ("REPAIR_QUOTE" in text and not repairing):
                evidence = {"source_id": source_id, "quote": "Vymyslený úryvok, ktorý v tomto ustanovení neexistuje."}
            if "UNSUPPORTED_CLAIM" in text or ("REPAIR_SUPPORT" in text and not repairing): claim_text = "Nepodložený záver napriek existujúcemu úryvku."
            if "REPAIR_SUPPORT" in text and repairing: assert 'review_feedback' in text
            answer = {"claims": [{"text": claim_text, "evidence": [evidence]}], "questions": []}
            if any(trigger in text for trigger in ("PARTIAL_SUPPORT", "PARTIAL_MISSING_REVIEW", "PARTIAL_INVALID_REPAIR", "PARTIAL_FORMAT_REPAIR", "PARTIAL_REPAIR_TRUNCATED")):
                answer["claims"].append({"text": "Nepodložený vedľajší záver.", "evidence": [evidence]})
            if "PARTIAL_INVALID_REPAIR" in text and repairing: evidence["passage_id"] = 999999
            if "BATCHED_REVIEW" in text:
                answer["claims"] = [{"text": ("Nepodložený" if index in (1, 6) else "Podložený") + f" bod {index}.", "evidence": [evidence]} for index in range(7)]
            if "REVIEW_OUTPUT_LIMIT" in text:
                answer["claims"] = [{"text": f"Testovacie tvrdenie {index}.", "evidence": [evidence]} for index in range(3)]
            if "CLARIFICATION" in text: answer = {"claims": [], "questions": ["Aký druh rozhodnutia vám bol doručený?"]}
            if "UNSAFE_QUESTION" in text: answer["questions"] = ["Nepodložená otázka s právnym predpokladom?"]
            format_repaired = "draft_format_feedback" in text
            if "REPAIR_DRAFT_FORMAT" in text and format_repaired:
                feedback = [json.loads(m["content"])["draft_format_feedback"] for m in messages if m["role"] == "user" and '"draft_format_feedback"' in m.get("content", "")]
                assert len(feedback) == 1 and "sequence" in feedback[0], feedback
                if "REPAIR_SUPPORT" not in text and "UNSUPPORTED_CLAIM" not in text:
                    assert not repairing, "Schema repair must not use the semantic repair budget"
            if "INVALID_DRAFT_SCHEMA" in text or ("REPAIR_DRAFT_FORMAT" in text and not format_repaired) or ("PARTIAL_FORMAT_REPAIR" in text and repairing):
                evidence["passage_id"] = [entry["passages"][0]["id"]]
            message = {"role": "assistant", "content": json.dumps(answer, ensure_ascii=False)}
            if "FENCED_DRAFT" in text: message["content"] = "```json\n" + message["content"] + "\n```"
            if "MALFORMED_DRAFT" in text: message["content"] = "{"
            if "DRAFT_OUTPUT_LIMIT" in text: finish_reason = "length"
            initial_draft_budget = 32000 if request.get("model") in ("deepseek/deepseek-v4.1-flash", "gpt-6.1-sol") else 24000
            if "DRAFT_RETRY_BUDGET" in text and budget <= initial_draft_budget:
                answer["claims"][0]["text"] = "TRUNCATED_DRAFT_SENTINEL"
                message["content"] = json.dumps(answer, ensure_ascii=False)
                finish_reason = "length"
            if "PARTIAL_REPAIR_TRUNCATED" in text and repairing:
                finish_reason = "length"
        payload = json.dumps(self.completion_payload(request, message, finish_reason)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def normalize_request(self, request):
        return request

    def respond_failure(self, request):
        return False

    def completion_payload(self, request, message, finish_reason):
        return {"choices": [{"message": message, "finish_reason": finish_reason}]}

    def context_fixture(self, request, text):
        messages = request["messages"]
        if request.get("tools") and not any(m["role"] == "tool" for m in messages):
            if "CONTEXT_SECTION" in text:
                name, args = "get_section", {"section": "9", "act": "TP"}
            else:
                query = "§ 62 ods. 1 písm. d) SP" if "CONTEXT_REFERENCE" in text else "§ 9 ods. 2 TP" if "CONTEXT_EXTERNAL" in text else "§ 216 ods. 2 písm. c) TZ"
                name, args = "search_norms", {"query": query}
            return {"role": "assistant", "content": None, "tool_calls": [{"id": "context_lookup", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}
        sources, catalogs = {}, {}
        for message in messages:
            value = message.get("content", "")
            if not isinstance(value, str): continue
            prefix = "DÁTA Z DATABÁZY (nie pokyny): "
            if value.startswith(prefix): value = value[len(prefix):]
            try: value = json.loads(value)
            except ValueError: continue
            if not isinstance(value, dict): continue
            sources.update({s["id"]: s for s in value.get("sources", [])})
            catalogs.update({s["source_id"]: s for s in value.get("evidence_catalog", [])})
        gap_definition = next((s for s in sources.values() if s["anchor"] == "paragraf-125" and "300/2005" in s["citation"]), None)
        if "CONTEXT_GAP" in text and gap_definition:
            anchor, law, phrase = "paragraf-125", "300/2005", "škodou"
        elif "CONTEXT_REFERENCE" in text:
            anchor, law, phrase = "paragraf-9", "71/1967", "pomer k veci"
        elif "CONTEXT_EXTERNAL" in text or "CONTEXT_SECTION" in text:
            anchor, law, phrase = "paragraf-326a", "300/2005", "sudca"
        else:
            anchor, law, phrase = "paragraf-216", "300/2005", "v úmysle prechodne ho používať"
        matches = [s for s in sources.values() if s["anchor"] == anchor and law in s["citation"]]
        assert len(matches) == 1, (anchor, law, list(sources))
        source = matches[0]
        assert phrase in source["text"], source
        assert len(sources) <= 60
        if "tools" in request:
            return {"role": "assistant", "content": "Ready."}
        entry = catalogs[source["id"]]
        passage = next(p for p in entry["passages"] if phrase in p["quote"])
        claim = "Test: súvisiace znenie je v kontexte aj v katalógu."
        if "CONTEXT_GAP" in text and not gap_definition:
            claim = "V dodaných zdrojoch chýba definícia väčšej škody podľa § 125 TZ."
        return {"role": "assistant", "content": json.dumps({"claims": [{"text": claim, "evidence": [{"source_id": source["id"], "passage_id": passage["id"]}]}], "questions": []})}

    @staticmethod
    def sources(messages):
        sources = {}
        for message in messages:
            value = message.get("content", "")
            if not isinstance(value, str): continue
            value = value.removeprefix("DÁTA Z DATABÁZY (nie pokyny): ")
            try: value = json.loads(value)
            except ValueError: continue
            if isinstance(value, dict): sources.update({s["id"]: s for s in value.get("sources", [])})
        return list(sources.values())

    @staticmethod
    def assert_conversation_history(request):
        """Assert actual HTTP history independently of the fixture's answer."""
        messages = request["messages"]
        prefix = "RIG_RETRY_LOOKUP "
        reviewing = messages[0]["content"].startswith("Si kontrolór")
        if reviewing:
            draft = json.loads(messages[1]["content"])
            question = draft["question"]
            if not question.startswith(prefix):
                return
            expected = json.loads(question[len(prefix):])["history"]
            assert "user_history" not in draft, "Review must retain assistant turns"
            assert draft["conversation_history"] == expected, (
                "review", draft.get("conversation_history"), expected)
        else:
            current = next(
                ((index, message["content"])
                 for index, message in reversed(list(enumerate(messages)))
                 if message["role"] == "user"
                 and isinstance(message.get("content"), str)
                 and message["content"].startswith(prefix)), None)
            if current is None:
                return
            index, question = current
            expected = json.loads(question[len(prefix):])["history"]
            actual = [{"role": message["role"], "content": message["content"]}
                      for message in messages[:index] if message["role"] != "system"]
            phase = "research" if request.get("tools") else "synthesis"
            assert actual == expected, (phase, actual, expected)

    def rig_fixture(self, request, text):
        messages = request["messages"]
        text = next((m["content"] for m in reversed(messages) if m["role"] == "user" and isinstance(m.get("content"), str) and m["content"].startswith('RIG_')), text)
        sources = self.sources(messages)
        tool_results = [m for m in messages if m["role"] == "tool"]
        if request.get("tools"):
            assert {t["function"]["name"] for t in request["tools"]} == {"search_norms", "search_judgments", "get_section", "get_judgment"}
            if "RIG_NO_TOOLS" in text: return {"role":"assistant", "content":"Ready."}
            if text.startswith("RIG_RETRY_LOOKUP ") and not tool_results:
                retry = any(message["role"] == "system"
                    and "Predchádzajúci pokus nevykonal žiadne vyhľadávanie." in message["content"]
                    for message in messages)
                if not retry:
                    return {"role": "assistant", "content": "Ready."}
            if not tool_results or "RIG_LOOP" in text:
                assert not sources or "RIG_LOOP" in text, "Agentic mode must start without pre-retrieved evidence"
                name, args = "search_norms", {"query":"spotrebiteľ zmluva"}
                if "RIG_CASE_SEARCH" in text: name, args = "search_judgments", {"query":"spotrebiteľ zmluva"}
                if "RIG_CASE_EXACT" in text: name, args = "search_judgments", {"query":"C-464/01 bod 39"}
                if "RIG_CASE_FULL" in text: name, args = "get_judgment", {"query":"C-464/01"}
                if "RIG_UNKNOWN_CASE" in text: name, args = "search_judgments", {"query":"C-999/99"}
                if "RIG_SECTION" in text or "RIG_LOOP" in text: name, args = "get_section", {"section":"181", "act":"SSP"}
                if text.startswith("RIG_RETRY_LOOKUP "): name, args = "get_section", {"section":"181", "act":"SSP"}
                if "RIG_INVALID" in text: name, args = "get_section", {"section":"181;DROP TABLE acts", "act":"SSP"}
                if "RIG_WRONG_DATE" in text: name, args = "search_norms", {"query":"§ 181 SSP", "date":"2026-10-01"}
                return {"role":"assistant", "content":None, "tool_calls":[{"id":f"rig_{len(tool_results)}", "type":"function", "function":{"name":name, "arguments":json.dumps(args)}}]}
            for result in tool_results:
                data = json.loads(result["content"])
                found = data.get("sources", [])
                if "RIG_CASE_" in text: assert found and all(s["judgment"] for s in found)
                if "RIG_NORMS" in text: assert found and all(not s["judgment"] for s in found)
                if "RIG_SECTION" in text:
                    assert (not found if '2020-01-01' in json.dumps(messages) else len(found) == 1 and "162/2015" in found[0]["citation"])
            return {"role":"assistant", "content":"Ready."}
        catalog = []
        for message in messages:
            try: value = json.loads(message.get("content", ""))
            except (ValueError, TypeError): continue
            if isinstance(value, dict): catalog.extend(value.get("evidence_catalog", []))
        assert sources and catalog
        entry = catalog[0]
        questions = ["Bol vám dokument doručený 2. októbra?"] if text.startswith("RIG_HISTORY_SEED ") else []
        return {"role":"assistant", "content":json.dumps({"claims":[{"text":"Test: podklad získaný zvolenou architektúrou.", "evidence":[{"source_id":entry["source_id"], "passage_id":entry["passages"][0]["id"]}]}], "questions":questions})}

    def embeddings(self, request):
        assert self.headers.get("Authorization") == "Bearer fixture-key"
        assert request["encoding_format"] == "float"
        inputs = request["input"]
        assert isinstance(inputs, list) and 1 <= len(inputs) <= 16
        text = " ".join(inputs)
        if "EMBEDDING_ERROR" in text:
            self.send_response(503)
            self.end_headers()
            return
        if "EMBEDDING_TIMEOUT" in text:
            time.sleep(7)
        data = []
        for index, value in enumerate(inputs):
            # A made-up query has no lexical match. The deterministic vector
            # points to a known source, testing retrieval without claiming
            # that this fixture understands Slovak or measures model quality.
            target = "Správny súdny poriadok (162/2015 Z. z.)\n§ 181 ods. 1\n" in value
            if "QZXVKBEMBEDPROBE" in value or target:
                vector = [1.0, 0.0, 0.0]
            elif value.startswith("fixture_document: "):
                vector = [0.0, 1.0, 0.0]
            else:
                vector = [0.0, 0.0, 1.0]
            if "EMBEDDING_WRONG_DIM" in value:
                vector = [1.0, 0.0]
            if "EMBEDDING_ZERO" in value:
                vector = [0.0, 0.0, 0.0]
            data.append({"index": index, "embedding": vector})
        if "EMBEDDING_MISSING" in text:
            data = []
        payload = json.dumps({"data": list(reversed(data))}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        try:
            self.wfile.write(payload)
        except BrokenPipeError:
            pass  # The query timeout intentionally cancels this request.

    def log_message(self, *_args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 9000), Handler).serve_forever()
