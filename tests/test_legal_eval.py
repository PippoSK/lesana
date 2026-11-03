import unittest
from legal_eval import matches, events

class BenchmarkTests(unittest.TestCase):
    def test_same_anchor_in_wrong_act_is_not_a_hit(self):
        expected = {"anchor": "paragraf-1", "act_path": "/SK/ZZ/2015/162/"}
        self.assertFalse(matches({"anchor": "paragraf-1", "source_url": "https://static.slov-lex.sk/static/SK/ZZ/2015/160/20260715.html"}, expected))
    def test_exact_lookup_cannot_get_credit_for_parent(self):
        expected = {"anchor": "paragraf-181.odsek-1", "act_path": "/SK/ZZ/2015/162/"}
        source = {"anchor": "paragraf-181", "source_url": "https://static.slov-lex.sk/static/SK/ZZ/2015/162/20260612.html"}
        self.assertFalse(matches(source, expected, exact=True))
        self.assertTrue(matches(source, expected))
    def test_sse_errors_are_not_answers(self):
        self.assertEqual(events('event: error\r\ndata: {"error":"failed"}\r\n\r\n'), [("error", {"error":"failed"})])

if __name__ == '__main__': unittest.main()
