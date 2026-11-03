"""Native OpenAI protocol contract fixture; never a legal-quality evaluation."""
from http.server import ThreadingHTTPServer
import json
import re
import threading
from mock_model import Handler
from mock_openrouter import OpenRouterHandler


class NativeOpenAIHandler(Handler):
    rate_limit_attempts = {}
    rate_limit_inputs = {}
    rate_limit_lock = threading.Lock()

    def do_GET(self):
        if self.path != "/fixture/rate-limits":
            self.send_error(404)
            return
        with self.rate_limit_lock:
            counts = [{"trigger": key[0], "case": key[1], "attempts": value}
                      for key, value in self.rate_limit_attempts.items()]
        payload = json.dumps(counts).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    @staticmethod
    def plain_text(message):
        content = message.get("content", "")
        return "".join(part.get("text", "") for part in content) if isinstance(content, list) else content

    def normalize_request(self, request):
        assert self.headers.get("Authorization") == "Bearer native-fixture-key"
        assert self.headers.get("HTTP-Referer") is None
        assert self.headers.get("X-Title") is None
        assert request["model"] == "gpt-6.1-sol"
        assert "provider" not in request, "Native requests must not contain OpenRouter routing"
        assert "TRUNCATED_RESEARCH_SENTINEL" not in json.dumps(request), "Truncated research must never enter retry history"
        self.raw_request = request
        assert self.path == "/v1/responses", "Every native Sol phase must use Responses to support max reasoning"
        assert not any(key in request for key in ("messages", "reasoning_effort", "response_format", "max_completion_tokens")), "Native Sol must use Responses request fields"
        assert request["store"] is False
        assert request.get("previous_response_id") is None
        assert request["reasoning"]["effort"] == "max"
        budget = request["max_output_tokens"]
        assert "reasoning.encrypted_content" in request["include"]
        inputs = request["input"]
        calls = {item["call_id"]: item for item in inputs if item["type"] == "function_call"}
        reasoning = {item["id"]: item for item in inputs if item["type"] == "reasoning"}
        messages = []
        if request.get("instructions"):
            messages.append({"role": "system", "content": request["instructions"]})
        for item in inputs:
            kind = item["type"]
            if kind == "message":
                messages.append({"role": item["role"], "content": item["content"]})
            elif kind == "function_call":
                messages.append({"role": "assistant", "content": None, "tool_calls": [{
                    "id": item["call_id"], "type": "function", "function": {
                        "name": item["name"], "arguments": item["arguments"]}}]})
            elif kind == "function_call_output":
                call_id = item["call_id"]
                assert call_id in calls, "Tool output lost its original call"
                assert calls[call_id]["id"] == "fc_" + call_id
                assert reasoning["reason_" + call_id]["encrypted_content"] == "fixture_encrypted_" + call_id
                messages.append({"role": "tool", "tool_call_id": call_id, "content": item["output"]})
            else:
                assert kind == "reasoning", kind
        assert any(message["role"] == "system" for message in messages)
        text = json.dumps(messages, ensure_ascii=False)
        normalized = {"messages": messages, "model": request["model"], "max_tokens": budget}
        if request.get("tools"):
            assert budget in (32000, 64000)
            if budget == 64000:
                assert any(trigger in text for trigger in ("RESEARCH_RETRY_BUDGET", "RIG_RESEARCH_TRUNCATED")), "Only truncated research should increase its budget"
            assert request.get("text", {}).get("format", {}).get("type", "text") == "text"
            normalized["tools"] = [{"type": "function", "function": {
                "name": tool["name"], "description": tool.get("description", ""),
                "parameters": tool["parameters"]}} for tool in request["tools"]]
        else:
            reviewing = self.plain_text(messages[0]).startswith("Si kontrolór")
            declared = request["text"]["format"]
            assert declared["type"] == "json_schema"
            assert declared["strict"] is True
            assert declared["name"] == ("lesana_evidence_review" if reviewing else "lesana_answer")
            expected = ({"claims": [{"index": "integer", "supported": "boolean", "reason": "string"}], "questions_safe": "boolean"}
                        if reviewing else {"claims": [{"text": "string", "evidence": [{"source_id": "integer", "passage_id": "integer"}]}], "questions": ["string"]})
            OpenRouterHandler.assert_closed_schema(declared["schema"], expected)
            if reviewing:
                assert budget in (24000, 48000), "Native Sol review needs bounded reasoning headroom"
                if budget == 48000:
                    assert any(trigger in text for trigger in ("SINGLE_REVIEW_HIGH_BUDGET", "REVIEW_ALWAYS_TRUNCATED", "REVIEW_OUTPUT_LIMIT")), "Only a truncated review should increase its budget"
            else:
                assert budget in (32000, 64000), "Native Sol answer retry must stay within 64000 tokens"
                if budget == 64000:
                    assert any(trigger in text for trigger in ("DRAFT_RETRY_BUDGET", "DRAFT_OUTPUT_LIMIT", "PARTIAL_REPAIR_TRUNCATED")), "Only a truncated answer should increase its budget"
        return normalized

    def respond_failure(self, request):
        messages = request["messages"]
        text = json.dumps(messages, ensure_ascii=False)
        phase = "RESEARCH" if request.get("tools") else "REVIEW" if self.plain_text(messages[0]).startswith("Si kontrolór") else "ANSWER"
        triggers = ("NATIVE_RATE_LIMIT_ALWAYS", "NATIVE_QUOTA_EXHAUSTED", "NATIVE_RETRY_" + phase)
        trigger = next((value for value in triggers if value in text), None)
        if trigger is None:
            return False
        if trigger in ("NATIVE_RATE_LIMIT_ALWAYS", "NATIVE_QUOTA_EXHAUSTED") and phase != "REVIEW":
            return False
        if phase == "RESEARCH" and any(message["role"] == "tool" for message in messages):
            return False
        nonce = re.search(r"RATECASE_[a-f0-9]+", text)
        assert nonce, "Rate-limit fixture needs a unique case identifier"
        key = (trigger, nonce.group())
        # The complete on-wire payload, including max effort and token budget,
        # must be identical when an HTTP request is retried.
        wire = json.dumps(self.raw_request, sort_keys=True)
        with self.rate_limit_lock:
            attempt = self.rate_limit_attempts.get(key, 0) + 1
            self.rate_limit_attempts[key] = attempt
            original = self.rate_limit_inputs.setdefault(key, wire)
        assert wire == original, "Rate-limit retry changed original inputs or model settings"
        if trigger.startswith("NATIVE_RETRY_") and attempt > 1:
            return False
        quota = trigger == "NATIVE_QUOTA_EXHAUSTED"
        # An extra retry changes the public error and makes the integration
        # assertion fail rather than silently tolerating an unbounded loop.
        status = 400 if attempt > (1 if quota else 4) else 429
        payload = json.dumps({"error": {
            "message": "Fixture quota exhausted" if quota else "Fixture transient rate limit",
            "type": "insufficient_quota" if quota else "tokens",
            "code": "insufficient_quota" if quota else "rate_limit_exceeded"}}).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Retry-After", "0")
        self.end_headers()
        self.wfile.write(payload)
        return True

    def completion_payload(self, request, message, finish_reason):
        researching = bool(request.get("tools"))
        if not researching:
            if "SCHEMA_EXTRA_FIELD" in json.dumps(request["messages"]):
                if not self.plain_text(request["messages"][0]).startswith("Si kontrolór"):
                    content = json.loads(message["content"])
                    content["unexpected_field"] = "Provider ignored the requested schema"
                    message = {**message, "content": json.dumps(content)}

        text = json.dumps(request["messages"])
        research_truncated = researching and ("RIG_RESEARCH_TRUNCATED" in text or (
            "RESEARCH_RETRY_BUDGET" in text and self.raw_request["max_output_tokens"] == 32000))
        truncated = research_truncated or finish_reason == "length"
        output = []
        for call in message.get("tool_calls", []):
            call_id = call["id"]
            output.append({"type": "reasoning", "id": "reason_" + call_id,
                "summary": [], "encrypted_content": "TRUNCATED_RESEARCH_SENTINEL" if truncated else "fixture_encrypted_" + call_id})
            output.append({"type": "function_call", "id": "fc_" + call_id,
                "status": "completed", "call_id": call_id, **call["function"]})
        if research_truncated:
            message = {**message, "content": "TRUNCATED_RESEARCH_SENTINEL"}
        if message.get("content"):
            output.append({"type": "message", "id": "msg_fixture", "status": "completed",
                "role": "assistant", "content": [{"type": "output_text", "text": message["content"], "annotations": []}]})
        return {"id": "resp_fixture", "object": "response", "created_at": 1,
            "model": self.raw_request["model"],
            "status": "incomplete" if truncated else "completed", "output": output,
            "error": None, "incomplete_details": {"reason": "max_output_tokens"} if truncated else None,
            "usage": {"input_tokens": 20, "output_tokens": 10, "total_tokens": 30}}


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 9000), NativeOpenAIHandler).serve_forever()
