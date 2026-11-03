"""OpenRouter protocol contract fixture; never a legal-quality evaluation."""
from http.server import ThreadingHTTPServer
import json
from mock_model import Handler


class OpenRouterHandler(Handler):
    @staticmethod
    def plain_text(message):
        content = message.get("content", "")
        return "".join(part.get("text", "") for part in content) if isinstance(content, list) else content

    @staticmethod
    def assert_closed_schema(schema, expected):
        if isinstance(expected, dict):
            assert schema["type"] == "object"
            assert schema["additionalProperties"] is False
            assert set(schema["properties"]) == set(expected)
            assert set(schema["required"]) == set(expected)
            for key, value in expected.items():
                OpenRouterHandler.assert_closed_schema(schema["properties"][key], value)
        elif isinstance(expected, list):
            assert schema["type"] == "array"
            OpenRouterHandler.assert_closed_schema(schema["items"], expected[0])
        else:
            assert schema["type"] == expected

    def normalize_request(self, request):
        assert self.headers.get("Authorization") == "Bearer fixture-key"
        assert request["model"] in ("openai/gpt-6.1-sol", "qwen/qwen3.8-27b", "deepseek/deepseek-v4.1-flash")
        qwen = request["model"] == "qwen/qwen3.8-27b"
        deepseek = request["model"] == "deepseek/deepseek-v4.1-flash"
        self.raw_request = request
        if self.path == "/api/v1/chat/completions":
            expected_effort = "xhigh" if qwen else "max" if deepseek else "low"
            assert request.get("reasoning_effort", request.get("reasoning", {}).get("effort")) == expected_effort
            budget = request.get("max_completion_tokens", request.get("max_tokens", 0))
            assert budget > 0
            if qwen:
                assert request["provider"] == {"require_parameters": True, "order": ["cerebras", "modelrun"], "sort": "throughput", "allow_fallbacks": True}
            if deepseek:
                assert request["provider"] == {"require_parameters": True, "order": ["coreweave/fp8", "together"], "only": ["together", "coreweave/fp8"], "allow_fallbacks": True}
                assert "TRUNCATED_RESEARCH_SENTINEL" not in json.dumps(request["messages"]), "Truncated research must never enter retry history"
                for message in request["messages"]:
                    if message.get("role") == "assistant" and message.get("tool_calls"):
                        details = message.get("reasoning_details", [])
                        assert any(item.get("text") == "fixture_deepseek_reasoning" for item in details), "DeepSeek tool continuation lost reasoning"
            if request.get("tools"):
                assert qwen or deepseek, "Sol tool calls must use Responses"
                if deepseek:
                    assert budget in (32000, 64000), "DeepSeek research retry must stay within 64000 tokens"
                    if budget == 64000:
                        text = json.dumps(request["messages"])
                        assert any(trigger in text for trigger in ("RESEARCH_RETRY_BUDGET", "RIG_RESEARCH_TRUNCATED")), "Only truncated research should increase its budget"
                else:
                    assert budget == 16384, "Chat research must have a bounded output budget"
                assert "response_format" not in request
            elif qwen or deepseek:
                reviewing = self.plain_text(request["messages"][0]).startswith("Si kontrolór")
                if deepseek:
                    if reviewing:
                        assert budget in (24000, 48000), "DeepSeek review needs bounded reasoning headroom"
                        if budget == 48000:
                            text = json.dumps(request["messages"], ensure_ascii=False)
                            assert any(trigger in text for trigger in ("SINGLE_REVIEW_HIGH_BUDGET", "REVIEW_ALWAYS_TRUNCATED")), "Only a truncated single-claim review should increase its budget"
                    else:
                        assert budget in (32000, 64000), "DeepSeek answer retry must stay within 64000 tokens"
                        if budget == 64000:
                            text = json.dumps(request["messages"], ensure_ascii=False)
                            assert any(trigger in text for trigger in ("DRAFT_RETRY_BUDGET", "DRAFT_OUTPUT_LIMIT", "PARTIAL_REPAIR_TRUNCATED")), "Only an output-limit failure should increase the answer budget"
                response_format = request["response_format"]
                assert response_format["type"] == "json_schema"
                declared = response_format["json_schema"]
                assert declared["strict"] is True
                assert declared["name"] == ("lesana_evidence_review" if reviewing else "lesana_answer")
                expected = ({"claims": [{"index": "integer", "supported": "boolean", "reason": "string"}], "questions_safe": "boolean"}
                            if reviewing else {"claims": [{"text": "string", "evidence": [{"source_id": "integer", "passage_id": "integer"}]}], "questions": ["string"]})
                self.assert_closed_schema(declared["schema"], expected)
                if deepseek:
                    properties = declared["schema"]["properties"]
                    claims = properties["claims"]
                    claim_fields = claims["items"]["properties"]
                    if reviewing:
                        assert claims["maxItems"] == 1, "DeepSeek reviews one claim per call"
                        assert claim_fields["reason"]["maxLength"] == 250
                    else:
                        assert claims["maxItems"] == 6, "Bound DeepSeek answer size before generation"
                        assert claim_fields["text"]["maxLength"] == 1800
                        assert claim_fields["evidence"]["minItems"] == 1
                        assert claim_fields["evidence"]["maxItems"] == 6
                        assert properties["questions"]["maxItems"] == 4
                        assert properties["questions"]["items"]["maxLength"] == 500
            else:
                assert request["response_format"] == {"type": "json_object"}
            return request
        assert not (qwen or deepseek), "Qwen and DeepSeek research must use Chat"
        assert self.path == "/api/v1/responses", self.path
        assert request["store"] is False
        assert request.get("previous_response_id") is None
        assert request["reasoning"]["effort"] == "low"
        assert request["max_output_tokens"] == 12000
        assert "reasoning.encrypted_content" in request["include"]
        assert not request.get("instructions"), "OpenRouter requires system input messages"
        inputs = request["input"]
        assert any(item.get("role") == "system" for item in inputs)
        calls = {item["call_id"]: item for item in inputs if item["type"] == "function_call"}
        reasoning = {item["id"]: item for item in inputs if item["type"] == "reasoning"}
        messages = []
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
        tools = [{"type": "function", "function": {
            "name": tool["name"], "description": tool.get("description", ""),
            "parameters": tool["parameters"]}} for tool in request["tools"]]
        return {"messages": messages, "tools": tools, "model": request["model"]}

    def completion_payload(self, request, message, finish_reason):
        if self.path == "/api/v1/chat/completions":
            if request["model"] == "deepseek/deepseek-v4.1-flash" and message.get("tool_calls"):
                message = {**message, "reasoning_details": [{"type": "reasoning.text", "text": "fixture_deepseek_reasoning", "format": "unknown", "index": 0}]}
            if request.get("tools") and "RIG_RESEARCH_TRUNCATED" in json.dumps(request["messages"]):
                finish_reason = "length"
            if request.get("tools") and "RESEARCH_RETRY_BUDGET" in json.dumps(request["messages"]):
                budget = request.get("max_completion_tokens", request.get("max_tokens", 0))
                if budget <= (32000 if request["model"] == "deepseek/deepseek-v4.1-flash" else 16384):
                    finish_reason = "length"
                    message = {**message, "content": "TRUNCATED_RESEARCH_SENTINEL"}
            if not request.get("tools") and "SCHEMA_EXTRA_FIELD" in json.dumps(request["messages"]):
                if not self.plain_text(request["messages"][0]).startswith("Si kontrolór"):
                    content = json.loads(message["content"])
                    content["unexpected_field"] = "Provider ignored the requested schema"
                    message = {**message, "content": json.dumps(content)}
            return super().completion_payload(request, message, finish_reason)
        output = []
        for call in message.get("tool_calls", []):
            call_id = call["id"]
            output.append({"type": "reasoning", "id": "reason_" + call_id,
                "summary": [], "encrypted_content": "fixture_encrypted_" + call_id})
            output.append({"type": "function_call", "id": "fc_" + call_id,
                "status": "completed", "call_id": call_id, **call["function"]})
        if message.get("content"):
            output.append({"type": "message", "id": "msg_fixture", "status": "completed",
                "role": "assistant", "content": [{"type": "output_text", "text": message["content"], "annotations": []}]})
        truncated = "RIG_RESEARCH_TRUNCATED" in json.dumps(request["messages"])
        return {"id": "resp_fixture", "object": "response", "created_at": 1,
            "model": self.raw_request["model"], "provider": "OpenAI",
            "status": "incomplete" if truncated else "completed", "output": output,
            "error": None, "incomplete_details": {"reason": "max_output_tokens"} if truncated else None,
            "usage": {"input_tokens": 20, "output_tokens": 10, "total_tokens": 30}}


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 9000), OpenRouterHandler).serve_forever()
