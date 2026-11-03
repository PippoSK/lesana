"""HTTP regression checks for conversation attachments, using only the standard library.

TEST_PASSWORD=test-only-password python3 -B tests/attachments.py http://localhost:8089
Append ``agent`` to also exercise both chat architectures with the protocol model.
For strict assertions that files reach research, synthesis and review, run
``python3 -B tests/attachments.py --model 9000`` as the model fixture, point
LLM_BASE_URL at its /v1 endpoint, and set ATTACHMENT_MODEL_URL to its base URL
when running the checks with ``agent``.
Every conversation created here is deleted, including after a failed assertion.
"""
import datetime
import http.cookiejar
import io
import json
import os
import sys
import urllib.error
import urllib.request
import uuid
import zipfile
from xml.sax.saxutils import escape


BASE = (sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8088").rstrip("/")
PASSWORD = os.environ.get("TEST_PASSWORD", "")
AGENT = len(sys.argv) > 2 and sys.argv[2] == "agent"
DOCX_TYPE = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
DOCUMENT_TEXT = "ATTACHMENT_CASE_FACT: Decision delivered 12 October 2026."
PRESENT = "ATTACHMENT_PROTOCOL_PRESENT"
ABSENT = "ATTACHMENT_PROTOCOL_ABSENT"


def assert_model_attachments(request):
    """Inspect real model wire messages without depending on the app's internals."""
    messages = []
    for message in request["messages"]:
        content = message.get("content") or ""
        if isinstance(content, list):
            content = "".join(part.get("text", "") for part in content)
        messages.append({"role": message["role"], "content": content})
    reviewing = messages[0]["content"].startswith("Si kontrolór")
    if reviewing:
        question = json.loads(messages[1]["content"])["question"]
    else:
        question = next((message["content"] for message in reversed(messages)
                         if message["role"] == "user"
                         and message["content"].startswith((PRESENT, ABSENT))), "")
    if not question.startswith((PRESENT, ABSENT)):
        return None
    documents = []
    for message in messages:
        if message["role"] == "system":
            assert DOCUMENT_TEXT not in message["content"], "Attachment text must remain untrusted user data"
        try:
            payload = json.loads(message["content"])
        except ValueError:
            continue
        if isinstance(payload, dict) and "attachments" in payload:
            assert message["role"] == "user", message
            documents.extend(payload["attachments"])
    if question.startswith(PRESENT):
        assert len(documents) == 1, ("Document missing or duplicated in model input", documents)
        assert documents[0]["filename"] == "case.txt", documents
        assert documents[0]["text"] == DOCUMENT_TEXT, documents
    else:
        assert not documents, ("Deleted document still reaches the model", documents)
        assert all(DOCUMENT_TEXT not in message["content"] for message in messages), messages
    phase = "review" if reviewing else "research" if request.get("tools") else "synthesis"
    return ("present" if question.startswith(PRESENT) else "absent") + "_" + phase


def serve_model(port):
    from http.server import ThreadingHTTPServer
    from threading import Lock
    from mock_model import Handler

    counts, lock = {}, Lock()

    class AttachmentHandler(Handler):
        def normalize_request(self, model_request):
            phase = assert_model_attachments(model_request)
            if phase:
                with lock:
                    counts[phase] = counts.get(phase, 0) + 1
            return model_request

        def do_GET(self):
            if self.path != "/attachment-checks":
                self.send_error(404)
                return
            with lock:
                body = json.dumps(counts).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    ThreadingHTTPServer(("0.0.0.0", port), AttachmentHandler).serve_forever()


def model_checks():
    url = os.environ.get("ATTACHMENT_MODEL_URL")
    if not url:
        return None
    with urllib.request.urlopen(url.rstrip("/") + "/attachment-checks", timeout=10) as response:
        return json.load(response)


def client():
    return urllib.request.build_opener(
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))


def request(browser, path, method="GET", data=None, expected=200, headers=None, raw=False):
    request_headers = dict(headers or {})
    if data is not None and not isinstance(data, bytes):
        data = json.dumps(data).encode()
        request_headers.setdefault("Content-Type", "application/json")
    req = urllib.request.Request(BASE + path, method=method, headers=request_headers, data=data)
    try:
        response = browser.open(req, timeout=90)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        status, body, response_headers = response.status, response.read(), response.headers
    assert status == expected, (method, path, status, expected, body[:1000])
    if raw:
        return body, response_headers
    if "text/event-stream" in response_headers.get("Content-Type", ""):
        events = []
        for frame in body.decode().replace("\r\n", "\n").split("\n\n"):
            kind, payload = None, None
            for line in frame.splitlines():
                if line.startswith("event:"):
                    kind = line[6:].strip()
                if line.startswith("data:"):
                    payload = json.loads(line[5:])
            if kind:
                events.append((kind, payload))
        return events
    if not body:
        return None
    try:
        return json.loads(body)
    except ValueError:
        assert expected >= 400, (path, body[:1000])
        return {"error": body.decode(errors="replace")}


def multipart(files):
    boundary = "attachment-test-" + uuid.uuid4().hex
    body = bytearray()
    for field, filename, content_type, content in files:
        body.extend((f'--{boundary}\r\nContent-Disposition: form-data; name="{field}"; '
                     f'filename="{filename}"\r\nContent-Type: {content_type}\r\n\r\n').encode())
        body.extend(content)
        body.extend(b"\r\n")
    body.extend(f"--{boundary}--\r\n".encode())
    return bytes(body), {"Content-Type": "multipart/form-data; boundary=" + boundary}


def endpoint(conversation, attachment=None):
    path = f"/api/conversations/{conversation}/attachments"
    return path + "/" + attachment if attachment else path


def validate(record, filename=None, content=None):
    assert {"id", "filename", "content_type", "size_bytes", "text_chars"} <= record.keys(), record
    uuid.UUID(record["id"])
    assert isinstance(record["filename"], str) and record["filename"], record
    assert isinstance(record["content_type"], str) and record["content_type"], record
    assert isinstance(record["size_bytes"], int) and 0 < record["size_bytes"] <= 10 * 1024 * 1024, record
    assert isinstance(record["text_chars"], int) and 0 < record["text_chars"] <= 100000, record
    assert "text" not in record and "content" not in record and "bytes" not in record, record
    if filename is not None:
        assert record["filename"] == filename, record
    if content is not None:
        assert record["size_bytes"] == len(content), record
    if "created_at" in record:
        datetime.datetime.fromisoformat(record["created_at"].replace("Z", "+00:00"))


def upload(browser, conversation, filename, content, content_type="text/plain", expected=201, headers=None):
    body, multipart_headers = multipart([("file", filename, content_type, content)])
    result = request(browser, endpoint(conversation), "POST", body, expected,
                     {**multipart_headers, **(headers or {})})
    if expected == 201:
        validate(result, filename, content)
    else:
        assert result.get("error"), result
    return result


def listed(browser, conversation):
    records = request(browser, endpoint(conversation))
    assert isinstance(records, list), records
    for record in records:
        validate(record)
    identifiers = {record["id"] for record in records}
    assert len(identifiers) == len(records), records
    return {record["id"]: record for record in records}


def docx(text):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("[Content_Types].xml", '<?xml version="1.0"?>'
                         '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
                         '<Override PartName="/word/document.xml" '
                         'ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
                         '</Types>')
        archive.writestr("_rels/.rels", '<?xml version="1.0"?>'
                         '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                         '<Relationship Id="rId1" '
                         'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" '
                         'Target="word/document.xml"/></Relationships>')
        archive.writestr("word/document.xml", '<?xml version="1.0" encoding="UTF-8"?>'
                         '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
                         '<w:body><w:p><w:r><w:t>' + escape(text) +
                         '</w:t></w:r></w:p></w:body></w:document>')
    return output.getvalue()


def pdf(text):
    # A complete one-page PDF, including the cross-reference table, needs no library.
    escaped = text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
    stream = f"BT /F1 12 Tf 72 720 Td ({escaped}) Tj ET".encode("ascii")
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>",
               b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
               b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
               f"<< /Length {len(stream)} >>\nstream\n".encode() + stream + b"\nendstream"]
    result, offsets = bytearray(b"%PDF-1.4\n"), [0]
    for index, value in enumerate(objects, 1):
        offsets.append(len(result))
        result.extend(f"{index} 0 obj\n".encode() + value + b"\nendobj\n")
    xref = len(result)
    result.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        result.extend(f"{offset:010d} 00000 n \n".encode())
    result.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    return bytes(result)


def main():
    first, second, anonymous = client(), client(), client()
    created, checks = [], []

    def conversation(browser=first):
        identifier = request(browser, "/api/conversations", "POST", {})["id"]
        created.append((browser, identifier))
        return identifier

    try:
        for browser in (first, second):
            request(browser, "/api/session", "POST", {"password": PASSWORD})
        primary, sibling, invalid = conversation(), conversation(), conversation()
        foreign = conversation(second)
        assert not listed(first, primary) and not listed(first, sibling)
        assert not listed(second, foreign)
        dummy = str(uuid.uuid4())
        for headers in ({}, {"Cookie": "lexa_session=invalid"}, {"Cookie": "lexa_session=" + dummy}):
            request(anonymous, endpoint(primary), expected=401, headers=headers)
            upload(anonymous, primary, "missing-auth.txt", b"Document", expected=401, headers=headers)
            request(anonymous, endpoint(primary, dummy), expected=401, headers=headers)
            request(anonymous, endpoint(primary, dummy), "DELETE", expected=401, headers=headers)
        wrong_origin = {"Origin": "https://untrusted.example"}
        upload(first, primary, "wrong-origin.txt", b"Document", expected=403, headers=wrong_origin)
        assert not listed(first, primary)
        checks.append("authentication and cross-origin upload protection")

        contents = "Príloha k podaniu: rozhodnutie bolo doručené 12. októbra 2026.\n".encode()
        text = upload(first, primary, "podanie.txt", contents)
        assert text["text_chars"] == len(contents.decode().strip()), text
        assert listed(first, primary) == {text["id"]: text}
        assert not listed(first, sibling) and not listed(second, foreign)
        for browser, identifier in ((second, primary), (first, foreign)):
            request(browser, endpoint(identifier), expected=404)
            upload(browser, identifier, "foreign.txt", b"Document", expected=404)
            request(browser, endpoint(identifier, text["id"]), expected=404)
            request(browser, endpoint(identifier, text["id"]), "DELETE", expected=404)
        for identifier, attachment in ((sibling, text["id"]), (primary, dummy), (dummy, text["id"])):
            request(first, endpoint(identifier, attachment), expected=404)
            request(first, endpoint(identifier, attachment), "DELETE", expected=404)
        request(first, endpoint(dummy), expected=404)
        upload(first, dummy, "missing.txt", b"Document", expected=404)
        request(first, endpoint(primary, text["id"]), "DELETE", expected=403, headers=wrong_origin)
        downloaded, headers = request(first, endpoint(primary, text["id"]), raw=True)
        assert downloaded == contents
        assert headers.get("Content-Type", "").startswith("text/plain"), headers
        assert headers.get("Content-Disposition", "").startswith("attachment;"), headers
        assert headers.get("X-Content-Type-Options") == "nosniff", headers
        checks.append("persistent metadata, exact original download, conversation and session isolation")

        for filename, content_type, content in (
                ("poznamky.md", "text/markdown", "# Poznámky\nZmluva bola podpísaná.\n".encode()),
                ("zmluva.docx", DOCX_TYPE, docx("Zmluva & príloha obsahujú dohodnutú lehotu.")),
                ("rozhodnutie.pdf", "application/pdf", pdf("Decision delivered on 12 October 2026."))):
            record = upload(first, primary, filename, content, content_type)
            assert request(first, endpoint(primary, record["id"]), raw=True)[0] == content
        fifth = upload(first, primary, "fifth.txt", b"Fifth document")
        assert len(listed(first, primary)) == 5
        upload(first, primary, "sixth.txt", b"Sixth document", expected=400)
        assert len(listed(first, primary)) == 5
        request(first, endpoint(primary, fifth["id"]), "DELETE", expected=204)
        request(first, endpoint(primary, fifth["id"]), expected=404)
        request(first, endpoint(primary, fifth["id"]), "DELETE", expected=404)
        replacement = upload(first, primary, "replacement.txt", b"Replacement document")
        assert replacement["id"] != fifth["id"] and len(listed(first, primary)) == 5
        checks.append("TXT, Markdown, DOCX and PDF extraction, file count limit and deletion")

        for filename, content_type, content in (
                ("program.exe", "application/octet-stream", b"Executable"),
                ("empty.txt", "text/plain", b""),
                ("blank.txt", "text/plain", b" \t\r\n"),
                ("invalid.txt", "text/plain", b"\xff\xfe\xfa"),
                ("binary.txt", "text/plain", b"a\x00b"),
                ("corrupt.pdf", "application/pdf", b"not a PDF"),
                ("blank.pdf", "application/pdf", pdf("")),
                ("corrupt.docx", DOCX_TYPE, b"not a ZIP archive"),
                ("blank.docx", DOCX_TYPE, docx("")),
                ("long.txt", "text/plain", b"x" * 100001)):
            upload(first, invalid, filename, content, content_type, expected=400)
            assert not listed(first, invalid), filename
        upload(first, invalid, "oversized.txt", b"x" * (10 * 1024 * 1024 + 1), expected=413)
        for fields in ([], [("wrong", "one.txt", "text/plain", b"One")],
                       [("file", "one.txt", "text/plain", b"One"),
                        ("file", "two.txt", "text/plain", b"Two")]):
            body, headers = multipart(fields)
            request(first, endpoint(invalid), "POST", body, expected=400, headers=headers)
            assert not listed(first, invalid), fields
        checks.append("unsupported, empty, corrupt, binary and oversized files; atomic multipart validation")

        budget = conversation()
        large = upload(first, budget, "large.txt", b"a" * 60000)
        upload(first, budget, "remaining.txt", b"b" * 40000)
        assert sum(record["text_chars"] for record in listed(first, budget).values()) == 100000
        upload(first, budget, "excess.txt", b"c", expected=400)
        assert len(listed(first, budget)) == 2
        request(first, endpoint(budget, large["id"]), "DELETE", expected=204)
        unicode_text = "ž" * 50001
        unicode_record = upload(first, budget, "unicode.txt", unicode_text.encode())
        assert unicode_record["text_chars"] == len(unicode_text), unicode_record
        checks.append("aggregate extracted-text limit, Unicode character counting and reclaimed capacity")

        if AGENT:
            before = model_checks()
            for architecture in ("pipeline", "agentic"):
                chat = conversation()
                document = upload(first, chat, "case.txt", DOCUMENT_TEXT.encode())
                for turn, trigger in enumerate((PRESENT, ABSENT), 1):
                    if trigger == ABSENT:
                        request(first, endpoint(chat, document["id"]), "DELETE", expected=204)
                    question = trigger + " Aká je lehota podľa § 181 SSP? Zohľadni aktuálne prílohy."
                    stream = request(first, f"/api/conversations/{chat}/chat", "POST",
                                     {"message": question, "architecture": architecture})
                    errors = [payload for kind, payload in stream if kind == "error"]
                    answers = [payload for kind, payload in stream if kind == "answer"]
                    assert not errors and len(answers) == 1, stream
                    assert answers[0]["mode"] == "agent" and answers[0]["claims"], answers
                    history = request(first, f"/api/conversations/{chat}")
                    assert len(history) == 2 * turn and history[-2]["content"] == question, history
                    assert all(DOCUMENT_TEXT not in message["content"] for message in history), history
                    assert listed(first, chat) == ({document["id"]: document} if trigger == PRESENT else {})
            checks.append("both chat architectures accept attached documents and preserve the original question")
            if before is not None:
                after = model_checks()
                for state in ("present", "absent"):
                    for phase in ("research", "synthesis", "review"):
                        key = state + "_" + phase
                        assert after.get(key, 0) > before.get(key, 0), (key, before, after)
                checks.append("model receives full attachment text during research, synthesis and review, until deletion")

        request(first, f"/api/conversations/{primary}", "DELETE", expected=204)
        created.remove((first, primary))
        request(first, endpoint(primary), expected=404)
        request(first, endpoint(primary, text["id"]), expected=404)
        checks.append("deleted conversations no longer expose their attachments")
    finally:
        for browser, identifier in reversed(created):
            request(browser, f"/api/conversations/{identifier}", "DELETE", expected=204)
    print(json.dumps({"status": "passed", "checks": checks}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--model":
        serve_model(int(sys.argv[2]) if len(sys.argv) > 2 else 9000)
    else:
        main()
