import copy
import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("compare", os.path.join(HERE, "..", "compare.py"))
compare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compare)


def doc(rps=1000.0, p99=20.0, errors=0, klass="test-class"):
    return {
        "schema_version": 1,
        "hardware": {"class": klass},
        "results": [
            {"endpoint": "/c", "fsync": "always", "median_rps": rps, "median_p99_ms": p99, "errors_total": errors},
            {"endpoint": "/c", "fsync": "off", "median_rps": rps * 10, "median_p99_ms": p99 / 10, "errors_total": errors},
        ],
    }


class CompareTests(unittest.TestCase):
    def test_passes_within_tolerance(self):
        code, f, w, _ = compare.evaluate(doc(rps=850, p99=29), doc())
        self.assertEqual((code, f, w), (0, [], []))

    def test_fails_rps_drop_over_20pct(self):
        code, f, _, _ = compare.evaluate(doc(rps=790), doc())
        self.assertEqual(code, 1)
        self.assertTrue(any("rps" in x for x in f))

    def test_fails_p99_regression(self):
        code, f, _, _ = compare.evaluate(doc(p99=31), doc())
        self.assertEqual(code, 1)
        self.assertTrue(any("p99" in x for x in f))

    def test_fails_on_any_errors(self):
        code, f, _, _ = compare.evaluate(doc(errors=1), doc())
        self.assertEqual(code, 1)
        self.assertTrue(any("error" in x for x in f))

    def test_missing_baseline_class_exits_3(self):
        code, _, _, _ = compare.evaluate(doc(klass="other"), doc())
        self.assertEqual(code, 3)
        self.assertEqual(compare.main(["x.json", "/nonexistent/baseline.json"]), 3)

    def test_noisy_runner_flag_downgrades_to_warning(self):
        code, f, w, _ = compare.evaluate(doc(rps=500, p99=100), doc(), noisy=True)
        self.assertEqual((code, f), (0, []))
        self.assertEqual(len(w), 4)  # 2 endpoints x (rps, p99)

    def test_noisy_runner_still_fails_on_errors(self):
        code, _, _, _ = compare.evaluate(doc(errors=3), doc(), noisy=True)
        self.assertEqual(code, 1)

    def test_missing_endpoint_in_result_fails(self):
        cur = doc()
        del cur["results"][1]
        code, f, _, _ = compare.evaluate(cur, doc())
        self.assertEqual(code, 1)
        self.assertTrue(any("missing" in x for x in f))


if __name__ == "__main__":
    unittest.main()
